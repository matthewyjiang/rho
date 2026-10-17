use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;

use super::{
    definition::{AgentDefinition, AgentFingerprint, AgentId},
    internal::{internal_definitions, is_internal_agent_id},
    parser::parse_definition,
};
use crate::plugins::{PluginAgentFiles, PluginAgentTiers};
use crate::workspace::ProjectTrust;

/// The built-in root agent. It is never offered for delegation.
pub(crate) const DEFAULT_AGENT_ID: &str = "default";

const BUILTINS: &[(&str, &str)] = &[
    (
        DEFAULT_AGENT_ID,
        include_str!("../builtin_agents/default.md"),
    ),
    ("explorer", include_str!("../builtin_agents/explorer.md")),
    ("reviewer", include_str!("../builtin_agents/reviewer.md")),
    ("worker", include_str!("../builtin_agents/worker.md")),
];

/// Source kind, ordered from lowest to highest discovery precedence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentOrigin {
    Internal,
    BuiltIn,
    /// Agents an active user Agent Plugin (`~/.agents/plugins`) ships in
    /// Rho's extension directory.
    UserPlugin,
    /// Agents an active, trusted project Agent Plugin ships; frozen workflows
    /// re-check plugin trust before running them.
    ProjectPlugin,
    AgentsHome,
    RhoHome,
    Project,
    /// Agents shipped beside a workflow entry (`<workflow_dir>/agents/*.md`).
    Workflow,
}

impl AgentOrigin {
    /// Stable snake_case name for structured output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::BuiltIn => "built_in",
            Self::UserPlugin => "user_plugin",
            Self::ProjectPlugin => "project_plugin",
            Self::AgentsHome => "agents_home",
            Self::RhoHome => "rho_home",
            Self::Project => "project",
            Self::Workflow => "workflow",
        }
    }

    /// Whether an Agent Plugin, of either scope, shipped the agent.
    pub fn is_plugin(self) -> bool {
        match self {
            Self::UserPlugin | Self::ProjectPlugin => true,
            Self::Internal
            | Self::BuiltIn
            | Self::AgentsHome
            | Self::RhoHome
            | Self::Project
            | Self::Workflow => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCatalogMetadata {
    pub origin: AgentOrigin,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCatalogEntry {
    pub definition: AgentDefinition,
    pub fingerprint: AgentFingerprint,
    pub metadata: AgentCatalogMetadata,
}

/// Loaded agents plus the user definition files discovery skipped.
///
/// One bad file must not stop startup, so user tiers skip files that fail to
/// read or parse and record why. The id a skipped file's name claims stays
/// unavailable from that tier down: `find` reports the file's error instead of
/// quietly falling back to a lower-precedence agent the user meant to override.
/// Built-in definitions still fail discovery, since they ship with the binary.
#[derive(Clone, Debug, Default)]
pub struct AgentCatalog {
    entries: BTreeMap<AgentId, AgentCatalogEntry>,
    internal_entries: BTreeMap<AgentId, AgentCatalogEntry>,
    skipped: Vec<AgentCatalogError>,
    blocked: BTreeMap<AgentId, AgentCatalogError>,
}

/// A catalog discovered at startup, tagged with the cwd it was walked from.
///
/// Discovery walks the project ancestor chain, so a catalog is only valid for
/// the directory it was discovered from. Resume can move the session cwd;
/// [`Self::for_cwd`] hands the catalog back only when the directories match,
/// and consumers rediscover otherwise.
#[derive(Clone, Debug)]
pub struct DiscoveredAgentCatalog {
    cwd: PathBuf,
    catalog: Arc<AgentCatalog>,
}

impl DiscoveredAgentCatalog {
    pub fn new(cwd: PathBuf, catalog: Arc<AgentCatalog>) -> Self {
        Self { cwd, catalog }
    }

    pub fn for_cwd(&self, cwd: &Path) -> Option<Arc<AgentCatalog>> {
        (self.cwd == cwd).then(|| Arc::clone(&self.catalog))
    }
}

#[derive(Default)]
pub(crate) struct AgentCatalogSources {
    pub(crate) user_plugins: Vec<AgentSource>,
    pub(crate) project_plugins: Vec<AgentSource>,
    pub(crate) agents_home: Vec<AgentSource>,
    pub(crate) rho_home: Vec<AgentSource>,
    pub(crate) project: Vec<Vec<AgentSource>>,
    pub(crate) workflow: Vec<AgentSource>,
}

/// One agent file a caller read itself, under whatever read policy it
/// enforces (the workflow tool gates the directories users own).
pub(crate) struct AgentSource {
    pub(crate) path: PathBuf,
    /// The file's text, or why reading it failed. A failed read skips only
    /// that file, as it does in discovery.
    pub(crate) contents: Result<String, String>,
}

impl AgentSource {
    pub(crate) fn read(path: PathBuf, contents: String) -> Self {
        Self {
            path,
            contents: Ok(contents),
        }
    }
}

impl AgentCatalog {
    /// Discovers built-ins, plugin agents, and user/project files. Precedence
    /// is project, `~/.rho/agents`, `~/.agents/agents`, project plugins, user
    /// plugins, then built-ins. Plugin agents cannot claim built-in ids.
    pub fn discover(cwd: &Path) -> Result<Self, AgentCatalogError> {
        let home = crate::paths::home_dir();
        let trust = ProjectTrust::from_agents_env();
        let plugins = crate::plugins::agent_tiers(cwd, home.as_deref());
        Self::discover_with_home_and_trust(cwd, home.as_deref(), trust, &plugins)
    }

    #[cfg(test)]
    pub fn discover_with_home(cwd: &Path, home: Option<&Path>) -> Result<Self, AgentCatalogError> {
        Self::discover_with_home_and_trust(
            cwd,
            home,
            ProjectTrust::Trusted,
            &PluginAgentTiers::default(),
        )
    }

    /// `project_trust` gates project `.agents/agents` files. `plugins` holds
    /// the active plugins' agent files (see [`crate::plugins::agent_tiers`]),
    /// which already follow plugin enable state and plugin trust, as every
    /// other component of a plugin does.
    pub fn discover_with_home_and_trust(
        cwd: &Path,
        home: Option<&Path>,
        project_trust: ProjectTrust,
        plugins: &PluginAgentTiers,
    ) -> Result<Self, AgentCatalogError> {
        let mut catalog = Self::default();
        catalog.load_builtins()?;
        for (origin, tier) in [
            (AgentOrigin::UserPlugin, &plugins.user),
            (AgentOrigin::ProjectPlugin, &plugins.project),
        ] {
            let sources = tier.iter().flat_map(PluginAgentFiles::read_sources);
            catalog.load_sources(origin, sources.collect());
        }
        if let Some(home) = home {
            let [agents_home, rho_home] = crate::paths::user_agent_dirs(home);
            catalog.load_tier(AgentOrigin::AgentsHome, &[agents_home]);
            catalog.load_tier(AgentOrigin::RhoHome, &[rho_home]);
        }
        if project_trust.is_trusted() {
            let project_roots: Vec<_> = crate::workspace::project_ancestor_dirs(cwd)
                .into_iter()
                .map(|path| path.join(".agents/agents"))
                .collect();
            catalog.load_tier(AgentOrigin::Project, &project_roots);
        }
        catalog.load_internals();
        Ok(catalog)
    }

    /// Catalog for planning one workflow entry, including `<dir>/agents/*.md`.
    pub fn discover_for_workflow_entry(
        cwd: &Path,
        workflow_entry: &Path,
        home: Option<&Path>,
        project_trust: ProjectTrust,
        plugins: &PluginAgentTiers,
    ) -> Result<Self, AgentCatalogError> {
        let mut catalog = Self::discover_with_home_and_trust(cwd, home, project_trust, plugins)?;
        catalog.load_tier(
            AgentOrigin::Workflow,
            &[workflow_local_agents_root(workflow_entry)],
        );
        Ok(catalog)
    }

    /// Builds a catalog from file bytes the caller read, instead of reading
    /// the filesystem here.
    pub(crate) fn from_sources(sources: AgentCatalogSources) -> Result<Self, AgentCatalogError> {
        let mut catalog = Self::default();
        catalog.load_builtins()?;
        catalog.load_sources(AgentOrigin::UserPlugin, sources.user_plugins);
        catalog.load_sources(AgentOrigin::ProjectPlugin, sources.project_plugins);
        catalog.load_sources(AgentOrigin::AgentsHome, sources.agents_home);
        catalog.load_sources(AgentOrigin::RhoHome, sources.rho_home);
        for tier in sources.project {
            catalog.load_sources(AgentOrigin::Project, tier);
        }
        catalog.load_sources(AgentOrigin::Workflow, sources.workflow);
        catalog.load_internals();
        Ok(catalog)
    }

    pub fn find(&self, id: &str) -> Result<&AgentCatalogEntry, AgentCatalogError> {
        let id = AgentId::new(id).map_err(|error| {
            AgentCatalogError::at_field(PathBuf::from("<selection>"), "id", error.to_string())
        })?;
        if let Some(entry) = self.entries.get(&id) {
            return Ok(entry);
        }
        if let Some(error) = self.blocked.get(&id) {
            return Err(error.clone());
        }
        Err(AgentCatalogError::at_field(
            PathBuf::from("<selection>"),
            "id",
            format!("unknown agent '{id}'"),
        ))
    }

    pub fn iter(&self) -> impl Iterator<Item = &AgentCatalogEntry> {
        self.entries.values()
    }

    pub fn iter_with_internal(&self) -> impl Iterator<Item = &AgentCatalogEntry> {
        self.internal_entries.values().chain(self.entries.values())
    }

    /// User definition files discovery skipped, in load order.
    pub fn skipped(&self) -> &[AgentCatalogError] {
        &self.skipped
    }

    fn load_builtins(&mut self) -> Result<(), AgentCatalogError> {
        let mut tier = TierLoad::new(AgentOrigin::BuiltIn);
        for (id, contents) in BUILTINS {
            let path = PathBuf::from(format!("<builtin:{id}>"));
            let definition = parse_definition(&path, id, contents)?;
            tier.insert(definition, path)?;
        }
        self.merge_tier(tier);
        Ok(())
    }

    fn load_tier(&mut self, origin: AgentOrigin, roots: &[PathBuf]) {
        let mut paths = Vec::new();
        let mut tier = TierLoad::new(origin);
        for root in roots {
            match markdown_paths(root) {
                Ok(found) => paths.extend(found),
                Err(error) => tier.skip(None, error),
            }
        }
        tier.read_files(paths);
        self.merge_tier(tier);
    }

    /// Loads one tier from files already located (and contained) by the caller.
    fn load_sources(&mut self, origin: AgentOrigin, sources: Vec<AgentSource>) {
        let mut tier = TierLoad::new(origin);
        for AgentSource { path, contents } in sources {
            match contents {
                Ok(contents) => tier.load_file(path, &contents),
                Err(error) => tier.skip_unreadable(path, error),
            }
        }
        self.merge_tier(tier);
    }

    fn load_internals(&mut self) {
        for definition in internal_definitions() {
            self.internal_entries.insert(
                definition.id.clone(),
                AgentCatalogEntry {
                    definition: definition.clone(),
                    fingerprint: definition.fingerprint(),
                    metadata: AgentCatalogMetadata {
                        origin: AgentOrigin::Internal,
                        path: None,
                    },
                },
            );
        }
    }

    fn merge_tier(&mut self, tier: TierLoad) {
        let origin = tier.origin;
        // A skipped file blocks its id from this tier down, unless a valid
        // file in this tier defines it.
        for (id, error) in tier.blocked {
            self.entries.remove(&id);
            self.blocked.insert(id, error);
        }
        for (id, (definition, path)) in tier.loaded {
            self.blocked.remove(&id);
            let fingerprint = definition.fingerprint();
            self.entries.insert(
                id,
                AgentCatalogEntry {
                    definition,
                    fingerprint,
                    metadata: AgentCatalogMetadata {
                        origin,
                        path: (origin != AgentOrigin::BuiltIn).then_some(path),
                    },
                },
            );
        }
        self.skipped.extend(tier.skipped);
    }
}

/// One precedence tier while it loads.
struct TierLoad {
    origin: AgentOrigin,
    loaded: BTreeMap<AgentId, (AgentDefinition, PathBuf)>,
    blocked: BTreeMap<AgentId, AgentCatalogError>,
    skipped: Vec<AgentCatalogError>,
}

impl TierLoad {
    fn new(origin: AgentOrigin) -> Self {
        Self {
            origin,
            loaded: BTreeMap::new(),
            blocked: BTreeMap::new(),
            skipped: Vec::new(),
        }
    }

    fn read_files(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            match std::fs::read_to_string(&path) {
                Ok(contents) => self.load_file(path, &contents),
                Err(error) => self.skip_unreadable(path, error.to_string()),
            }
        }
    }

    /// Skips a file that could not be read; the id it claims becomes
    /// unavailable from this tier down, as for a file that cannot parse.
    fn skip_unreadable(&mut self, path: PathBuf, error: String) {
        let id = stem_id(&path);
        self.skip(
            id,
            AgentCatalogError::at_path(path, format!("cannot read file: {error}")),
        );
    }

    /// Parses one user file, skipping it when it cannot load.
    fn load_file(&mut self, path: PathBuf, contents: &str) {
        let Some(fallback_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            let error = AgentCatalogError::at_field(path, "id", "filename is not valid UTF-8");
            self.skip(None, error);
            return;
        };
        let definition = match parse_definition(&path, fallback_id, contents) {
            Ok(definition) => definition,
            Err(error) => {
                self.skip(stem_id(&path), error);
                return;
            }
        };
        if let Some(reserved) = reserved_id(self.origin, &definition.id) {
            let id = &definition.id;
            let message = match reserved {
                ReservedId::Internal => format!(
                    "agent ID '{id}' is reserved for an internal agent and cannot be overridden"
                ),
                ReservedId::BuiltIn => format!(
                    "agent ID '{id}' is a built-in Rho agent; plugin agents cannot override built-ins"
                ),
                ReservedId::Host => {
                    format!("agent ID '{id}' is reserved by Rho; plugin agents cannot use it")
                }
            };
            self.skip(None, AgentCatalogError::at_field(path, "id", message));
            return;
        }
        if let Err(error) = self.insert(definition.clone(), path) {
            // Neither same-tier duplicate wins; the id stays unavailable.
            self.loaded.remove(&definition.id);
            self.skip(Some(definition.id), error);
        }
    }

    fn insert(
        &mut self,
        definition: AgentDefinition,
        path: PathBuf,
    ) -> Result<(), AgentCatalogError> {
        if self.blocked.contains_key(&definition.id) {
            return Err(AgentCatalogError::at_field(
                path,
                "id",
                format!(
                    "agent ID '{}' is also claimed by a file that failed to load at the same precedence",
                    definition.id
                ),
            ));
        }
        if let Some((_, first_path)) = self.loaded.get(&definition.id) {
            return Err(AgentCatalogError::at_field(
                path,
                "id",
                format!(
                    "duplicate agent ID '{}' at the same precedence; first defined in {}",
                    definition.id,
                    first_path.display()
                ),
            ));
        }
        self.loaded
            .insert(definition.id.clone(), (definition, path));
        Ok(())
    }

    /// Records a skipped file. `id` is the agent the file claims, when known;
    /// it becomes unavailable from this tier down. A plugin file never blocks
    /// a built-in id, since plugins cannot override built-ins.
    fn skip(&mut self, id: Option<AgentId>, error: AgentCatalogError) {
        tracing::warn!(error = %error, "skipping invalid agent definition");
        let blocks = |id: &AgentId| reserved_id(self.origin, id) != Some(ReservedId::BuiltIn);
        if let Some(id) = id.filter(blocks) {
            self.loaded.remove(&id);
            self.blocked.entry(id).or_insert_with(|| error.clone());
        }
        self.skipped.push(error);
    }
}

/// Id the synthetic `/side` agent binds under.
pub(crate) const SIDE_AGENT_ID: &str = "side";

/// An id Rho owns, so catalog files and model overrides cannot take it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReservedId {
    /// Internal agents, reserved from every tier.
    Internal,
    /// Built-in agents, reserved from plugins only.
    BuiltIn,
    /// Ids Rho uses outside the catalog (the permission screen's
    /// `[internal_agents]` key and the `/side` agent), reserved from plugins
    /// only.
    Host,
}

impl ReservedId {
    fn of(id: &AgentId) -> Option<Self> {
        let id = id.as_str();
        if is_internal_agent_id(id) {
            Some(Self::Internal)
        } else if BUILTINS.iter().any(|(builtin, _)| *builtin == id) {
            Some(Self::BuiltIn)
        } else if [
            crate::permission_classifier::DECISION_SCREEN_ID,
            SIDE_AGENT_ID,
        ]
        .contains(&id)
        {
            Some(Self::Host)
        } else {
            None
        }
    }
}

/// The reservation that stops a file from `origin` defining `id`. Only
/// internal ids are reserved from the files users own.
fn reserved_id(origin: AgentOrigin, id: &AgentId) -> Option<ReservedId> {
    match ReservedId::of(id)? {
        ReservedId::Internal => Some(ReservedId::Internal),
        reserved @ (ReservedId::BuiltIn | ReservedId::Host) => {
            origin.is_plugin().then_some(reserved)
        }
    }
}

/// Whether Rho owns `id`: an internal, built-in, or host agent id. Such ids
/// never take a catalog model override.
pub(crate) fn is_reserved_agent_id(id: &AgentId) -> bool {
    ReservedId::of(id).is_some()
}

/// The agent id a definition file's name claims, when it is a valid id.
fn stem_id(path: &Path) -> Option<AgentId> {
    let stem = path.file_stem()?.to_str()?;
    AgentId::new(stem).ok()
}

fn markdown_paths(root: &Path) -> Result<Vec<PathBuf>, AgentCatalogError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(AgentCatalogError::at_path(
                root.to_path_buf(),
                format!("cannot read agent directory: {error}"),
            ));
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            AgentCatalogError::at_path(root.to_path_buf(), format!("cannot read entry: {error}"))
        })?;
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|extension| extension == "md") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Directory that holds agents shipped with one workflow entry file.
///
/// Layout: `<parent-of-entry>/agents/*.md`.
pub fn workflow_local_agents_root(workflow_entry: &Path) -> PathBuf {
    match workflow_entry.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("agents"),
        _ => PathBuf::from("agents"),
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{path}{field_label}: {message}")]
pub struct AgentCatalogError {
    pub path: PathBuf,
    pub field: Option<String>,
    pub message: String,
    field_label: String,
}

impl AgentCatalogError {
    pub(super) fn at_path(path: PathBuf, message: impl Into<String>) -> Self {
        Self {
            path,
            field: None,
            message: message.into(),
            field_label: String::new(),
        }
    }

    pub(super) fn at_field(
        path: PathBuf,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let field = field.into();
        Self {
            path,
            field_label: format!(": field '{field}'"),
            field: Some(field),
            message: message.into(),
        }
    }
}
