use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use thiserror::Error;

use super::{
    definition::{AgentDefinition, AgentFingerprint, AgentId},
    parser::parse_definition,
};

const BUILTINS: &[(&str, &str)] = &[
    ("default", include_str!("../builtin_agents/default.md")),
    ("explorer", include_str!("../builtin_agents/explorer.md")),
    ("reviewer", include_str!("../builtin_agents/reviewer.md")),
    ("worker", include_str!("../builtin_agents/worker.md")),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectTrust {
    Trusted,
    Untrusted,
}

/// Source kind, ordered from lowest to highest discovery precedence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentOrigin {
    BuiltIn,
    AgentsHome,
    RhoHome,
    Project,
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

#[derive(Clone, Debug, Default)]
pub struct AgentCatalog {
    entries: BTreeMap<AgentId, AgentCatalogEntry>,
}

impl AgentCatalog {
    /// Discovers built-ins and user/project files. Precedence is project,
    /// `~/.rho/agents`, `~/.agents/agents`, then built-ins.
    pub fn discover(cwd: &Path) -> Result<Self, AgentCatalogError> {
        let home = crate::paths::home_dir();
        let trust = if std::env::var_os("RHO_TRUST_PROJECT_AGENTS").as_deref()
            == Some(std::ffi::OsStr::new("1"))
        {
            ProjectTrust::Trusted
        } else {
            ProjectTrust::Untrusted
        };
        Self::discover_with_home_and_trust(cwd, home.as_deref(), trust)
    }

    #[cfg(test)]
    pub fn discover_with_home(cwd: &Path, home: Option<&Path>) -> Result<Self, AgentCatalogError> {
        Self::discover_with_home_and_trust(cwd, home, ProjectTrust::Trusted)
    }

    pub fn discover_with_home_and_trust(
        cwd: &Path,
        home: Option<&Path>,
        project_trust: ProjectTrust,
    ) -> Result<Self, AgentCatalogError> {
        let mut catalog = Self::default();
        catalog.load_builtins()?;
        if let Some(home) = home {
            catalog.load_tier(AgentOrigin::AgentsHome, &[home.join(".agents/agents")])?;
            catalog.load_tier(AgentOrigin::RhoHome, &[home.join(".rho/agents")])?;
        }
        if project_trust == ProjectTrust::Trusted {
            let project_roots: Vec<_> = crate::workspace::project_ancestor_dirs(cwd)
                .into_iter()
                .map(|path| path.join(".agents/agents"))
                .collect();
            catalog.load_tier(AgentOrigin::Project, &project_roots)?;
        }
        Ok(catalog)
    }

    pub fn find(&self, id: &str) -> Result<&AgentCatalogEntry, AgentCatalogError> {
        let id = AgentId::new(id).map_err(|error| {
            AgentCatalogError::at_field(PathBuf::from("<selection>"), "id", error.to_string())
        })?;
        self.entries.get(&id).ok_or_else(|| {
            AgentCatalogError::at_field(
                PathBuf::from("<selection>"),
                "id",
                format!("unknown agent '{id}'"),
            )
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &AgentCatalogEntry> {
        self.entries.values()
    }

    fn load_builtins(&mut self) -> Result<(), AgentCatalogError> {
        let mut tier = BTreeMap::new();
        for (id, contents) in BUILTINS {
            let path = PathBuf::from(format!("<builtin:{id}>"));
            let definition = parse_definition(&path, id, contents)?;
            insert_in_tier(&mut tier, definition, &path)?;
        }
        self.merge_tier(tier, AgentOrigin::BuiltIn);
        Ok(())
    }

    fn load_tier(
        &mut self,
        origin: AgentOrigin,
        roots: &[PathBuf],
    ) -> Result<(), AgentCatalogError> {
        let mut tier = BTreeMap::new();
        for root in roots {
            for path in markdown_paths(root)? {
                let contents = std::fs::read_to_string(&path).map_err(|error| {
                    AgentCatalogError::at_path(path.clone(), format!("cannot read file: {error}"))
                })?;
                let fallback_id =
                    path.file_stem()
                        .and_then(|stem| stem.to_str())
                        .ok_or_else(|| {
                            AgentCatalogError::at_field(
                                path.clone(),
                                "id",
                                "filename is not valid UTF-8",
                            )
                        })?;
                let definition = parse_definition(&path, fallback_id, &contents)?;
                insert_in_tier(&mut tier, definition, &path)?;
            }
        }
        self.merge_tier(tier, origin);
        Ok(())
    }

    fn merge_tier(
        &mut self,
        tier: BTreeMap<AgentId, (AgentDefinition, PathBuf)>,
        origin: AgentOrigin,
    ) {
        for (id, (definition, path)) in tier {
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
    }
}

fn insert_in_tier(
    tier: &mut BTreeMap<AgentId, (AgentDefinition, PathBuf)>,
    definition: AgentDefinition,
    path: &Path,
) -> Result<(), AgentCatalogError> {
    if let Some((_, first_path)) = tier.get(&definition.id) {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "id",
            format!(
                "duplicate agent ID '{}' at the same precedence; first defined in {}",
                definition.id,
                first_path.display()
            ),
        ));
    }
    tier.insert(definition.id.clone(), (definition, path.to_path_buf()));
    Ok(())
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
