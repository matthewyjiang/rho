//! Agent definition check behind `rho(action="agents")`.
//!
//! Rediscovers the catalog from disk, so a definition written moments ago is
//! checked exactly as the next session or `agent` launch would load it. Also
//! names the directories a new definition can be written to, so the guided
//! creator never guesses `~` expansion or the project root.

use std::path::Path;

use serde::Serialize;

use super::AgentCatalog;
use crate::workspace::ProjectTrust;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AgentCheckReport {
    pub dirs: AgentDirs,
    #[serde(flatten)]
    pub status: AgentCheckStatus,
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
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum AgentCheckStatus {
    /// Every definition parsed. Lists file-backed agents only.
    Ok { agents: Vec<CheckedAgent> },
    /// Discovery stops at the first invalid file, as it does at startup.
    Error {
        path: String,
        field: Option<String>,
        message: String,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CheckedAgent {
    pub id: String,
    pub origin: &'static str,
    pub path: String,
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
    let status = match AgentCatalog::discover_with_home_and_trust(cwd, home, project_trust) {
        Ok(catalog) => AgentCheckStatus::Ok {
            agents: catalog
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
        },
        Err(error) => AgentCheckStatus::Error {
            path: crate::paths::display(&error.path),
            field: error.field,
            message: error.message,
        },
    };
    AgentCheckReport {
        dirs: AgentDirs {
            agents_home,
            rho_home,
            project,
        },
        status,
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
