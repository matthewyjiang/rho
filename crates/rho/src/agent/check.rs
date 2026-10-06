//! Agent definition check behind `rho(action="agents")`.
//!
//! Rediscovers the catalog from disk, so a definition written moments ago is
//! checked exactly as the next session or `agent` launch would load it. Also
//! names the directories a new definition can be written to, so the guided
//! creator never guesses `~` expansion or the project root.

use std::path::Path;

use serde::Serialize;

use super::{AgentCatalog, AgentCatalogError};
use crate::workspace::ProjectTrust;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AgentCheckReport {
    pub dirs: AgentDirs,
    /// File-backed agents that loaded.
    pub agents: Vec<CheckedAgent>,
    /// Files discovery skipped. Their agents are unavailable until fixed.
    pub invalid: Vec<InvalidAgentFile>,
}

/// Directories a user definition can be saved to. `None` when unavailable:
/// no home directory, or project agents are untrusted.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AgentDirs {
    pub agents_home: Option<String>,
    pub rho_home: Option<String>,
    pub project: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CheckedAgent {
    pub id: String,
    pub origin: &'static str,
    pub path: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct InvalidAgentFile {
    pub path: String,
    pub field: Option<String>,
    pub message: String,
}

impl From<AgentCatalogError> for InvalidAgentFile {
    fn from(error: AgentCatalogError) -> Self {
        Self {
            path: crate::paths::display(&error.path),
            field: error.field,
            message: error.message,
        }
    }
}

/// Checks every definition visible from `cwd`.
pub(crate) fn check_agents(
    cwd: &Path,
    home: Option<&Path>,
    project_trust: ProjectTrust,
) -> AgentCheckReport {
    let [agents_home, rho_home] = match home {
        Some(home) => {
            crate::paths::user_agent_dirs(home).map(|dir| Some(crate::paths::display(&dir)))
        }
        None => [None, None],
    };
    let project = project_trust
        .is_trusted()
        .then(|| {
            crate::workspace::project_ancestor_dirs(cwd)
                .into_iter()
                .next()
        })
        .flatten()
        .map(|root| crate::paths::display(&root.join(".agents/agents")));
    let (agents, invalid) =
        match AgentCatalog::discover_with_home_and_trust(cwd, home, project_trust) {
            Ok(catalog) => (
                catalog
                    .iter()
                    .filter_map(|entry| {
                        let path = entry.metadata.path.as_deref()?;
                        Some(CheckedAgent {
                            id: entry.definition.id.as_str().to_string(),
                            origin: entry.metadata.origin.as_str(),
                            path: crate::paths::display(path),
                        })
                    })
                    .collect(),
                catalog.skipped().iter().cloned().map(Into::into).collect(),
            ),
            // Only a broken built-in fails discovery outright.
            Err(error) => (Vec::new(), vec![error.into()]),
        };
    AgentCheckReport {
        dirs: AgentDirs {
            agents_home,
            rho_home,
            project,
        },
        agents,
        invalid,
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
