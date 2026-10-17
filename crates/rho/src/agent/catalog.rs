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
use crate::plugins::{PluginAgentFiles, PluginScope};
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
    /// Agents an active Agent Plugin ships in Rho's extension directory.
    Plugin,
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
            Self::Plugin => "plugin",
            Self::AgentsHome => "agents_home",
            Self::RhoHome => "rho_home",
            Self::Project => "project",
            Self::Workflow => "workflow",
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
    pub(crate) user_plugins: Vec<(PathBuf, String)>,
    pub(crate) project_plugins: Vec<(PathBuf, String)>,
    pub(crate) agents_home: Vec<(PathBuf, String)>,
    pub(crate) rho_home: Vec<(PathBuf, String)>,
    pub(crate) project: Vec<Vec<(PathBuf, String)>>,
    pub(crate) workflow: Vec<(PathBuf, String)>,
}

impl AgentCatalog {
    /// Discovers built-ins, plugin agents, and user/project files. Precedence
    /// is project, `~/.rho/agents`, `~/.agents/agents`, project plugins, user
    /// plugins, then built-ins. Plugin agents cannot claim built-in ids.
    pub fn discover(cwd: &Path) -> Result<Self, AgentCatalogError> {
        let home = crate::paths::home_dir();
        let trust = ProjectTrust::from_agents_env();
        Self::discover_with_home_and_trust(cwd, home.as_deref(), trust)
    }

    #[cfg(test)]
    pub fn discover_with_home(cwd: &Path, home: Option<&Path>) -> Result<Self, AgentCatalogError> {
        Self::discover_with_home_and_trust(cwd, home, ProjectTrust::Trusted)
    }

    /// `project_trust` gates project `.agents/agents` files. Project plugin
    /// agents follow plugin trust (`RHO_TRUST_PROJECT_PLUGINS`) instead, as
    /// every other component of that plugin does.
    pub fn discover_with_home_and_trust(
        cwd: &Path,
        home: Option<&Path>,
        project_trust: ProjectTrust,
    ) -> Result<Self, AgentCatalogError> {
        let mut catalog = Self::default();
        catalog.load_builtins()?;
        let plugin_agents = crate::plugins::agents_by_precedence(cwd, home);
        for scope in [PluginScope::User, PluginScope::Project] {
            catalog.load_paths(
                AgentOrigin::Plugin,
                plugin_agent_paths(&plugin_agents, scope),
            );
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
    ) -> Result<Self, AgentCatalogError> {
        let mut catalog = Self::discover_with_home_and_trust(cwd, home, project_trust)?;
        catalog.load_tier(
            AgentOrigin::Workflow,
            &[workflow_local_agents_root(workflow_entry)],
        );
        Ok(catalog)
    }

    /// Builds a catalog from file bytes whose reads the caller already authorized.
    pub(crate) fn from_authorized_sources(
        sources: AgentCatalogSources,
    ) -> Result<Self, AgentCatalogError> {
        let mut catalog = Self::default();
        catalog.load_builtins()?;
        catalog.load_sources(AgentOrigin::Plugin, sources.user_plugins);
        catalog.load_sources(AgentOrigin::Plugin, sources.project_plugins);
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
    fn load_paths(&mut self, origin: AgentOrigin, paths: Vec<PathBuf>) {
        let mut tier = TierLoad::new(origin);
        tier.read_files(paths);
        self.merge_tier(tier);
    }

    fn load_sources(&mut self, origin: AgentOrigin, sources: Vec<(PathBuf, String)>) {
        let mut tier = TierLoad::new(origin);
        for (path, contents) in sources {
            tier.load_file(path, &contents);
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
                Err(error) => {
                    let error = AgentCatalogError::at_path(
                        path.clone(),
                        format!("cannot read file: {error}"),
                    );
                    self.skip(stem_id(&path), error);
                }
            }
        }
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
        if is_internal_agent_id(&definition.id) {
            let error = AgentCatalogError::at_field(
                path,
                "id",
                format!(
                    "agent ID '{}' is reserved for an internal agent and cannot be overridden",
                    definition.id
                ),
            );
            self.skip(None, error);
            return;
        }
        if self.origin == AgentOrigin::Plugin && is_builtin_id(&definition.id) {
            let error = AgentCatalogError::at_field(
                path,
                "id",
                format!(
                    "agent ID '{}' is a built-in Rho agent; plugin agents cannot override built-ins",
                    definition.id
                ),
            );
            self.skip(None, error);
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
        let blocks = |id: &AgentId| !(self.origin == AgentOrigin::Plugin && is_builtin_id(id));
        if let Some(id) = id.filter(blocks) {
            self.loaded.remove(&id);
            self.blocked.entry(id).or_insert_with(|| error.clone());
        }
        self.skipped.push(error);
    }
}

fn is_builtin_id(id: &AgentId) -> bool {
    BUILTINS.iter().any(|(builtin, _)| *builtin == id.as_str())
}

/// One scope's plugin agent files; the catalog loads each scope as a tier.
fn plugin_agent_paths(plugins: &[PluginAgentFiles], scope: PluginScope) -> Vec<PathBuf> {
    plugins
        .iter()
        .filter(|plugin| plugin.scope == scope)
        .flat_map(|plugin| plugin.files.iter().cloned())
        .collect()
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
