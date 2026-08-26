//! Host-owned create/replace for user agent definition files.
//!
//! Callers collect a draft through any UX (questionnaire, editor, tests). This
//! path always parses, canonicalizes, authorizes a discovery root, and writes
//! under the shared agent save lock.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{
    authorize::{authorize_agent_destination, origin_roots},
    edit::{
        acquire_agent_file_lock, canonical_definition_contents, read_current_agent_file,
        write_agent_file,
    },
    parse_definition, AgentOrigin, SaveDefinitionError,
};
use crate::workspace::ProjectTrust;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentSaveLocation {
    AgentsHome,
    RhoHome,
    Project,
}

impl AgentSaveLocation {
    fn origin(self) -> AgentOrigin {
        match self {
            Self::AgentsHome => AgentOrigin::AgentsHome,
            Self::RhoHome => AgentOrigin::RhoHome,
            Self::Project => AgentOrigin::Project,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistDefinitionOutcome {
    pub path: PathBuf,
    pub contents: String,
    pub created: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PersistDefinitionError {
    Validation(String),
    Unauthorized(String),
    Exists {
        path: PathBuf,
        contents: String,
        revision: String,
    },
    Conflict,
    Write(String),
}

impl std::fmt::Display for PersistDefinitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(message) => write!(formatter, "agent validation failed: {message}"),
            Self::Unauthorized(message) => write!(formatter, "{message}"),
            Self::Exists { path, .. } => {
                write!(
                    formatter,
                    "agent file already exists: {}",
                    crate::paths::display(path)
                )
            }
            Self::Conflict => write!(formatter, "agent file changed since editing began"),
            Self::Write(message) => write!(formatter, "could not write agent file: {message}"),
        }
    }
}

impl std::error::Error for PersistDefinitionError {}

impl From<SaveDefinitionError> for PersistDefinitionError {
    fn from(error: SaveDefinitionError) -> Self {
        match error {
            SaveDefinitionError::Validation(message) => Self::Validation(message),
            SaveDefinitionError::Conflict => Self::Conflict,
            SaveDefinitionError::Write(message) => Self::Write(message),
        }
    }
}

/// Parses `contents`, authorizes `location`, and writes the canonical file.
///
/// `expected_revision` must be the revision from a [`PersistDefinitionError::Exists`]
/// response. Under the save lock, a mismatch (including a missing file) is
/// [`PersistDefinitionError::Conflict`].
pub(crate) fn persist_definition(
    location: AgentSaveLocation,
    contents: &str,
    expected_revision: Option<&str>,
    cwd: &Path,
    home: Option<&Path>,
    project_trust: ProjectTrust,
) -> Result<PersistDefinitionOutcome, PersistDefinitionError> {
    let path = Path::new("<draft>");
    let draft = parse_definition(path, "draft", contents)
        .map_err(|error| PersistDefinitionError::Validation(error.to_string()))?;
    let dest = persist_destination_path(location, cwd, home, project_trust, draft.id.as_str())?;
    authorize_agent_destination(location.origin(), &dest, cwd, home)
        .map_err(|error| PersistDefinitionError::Unauthorized(error.to_string()))?;
    let contents = canonical_definition_contents(&draft, &dest)?;

    let _lock = acquire_agent_file_lock(&dest)?;
    let current = read_current_agent_file(&dest)?;
    let created = match (expected_revision, current.as_deref()) {
        (None, None) => true,
        (None, Some(existing)) => {
            return Err(PersistDefinitionError::Exists {
                path: dest,
                revision: content_revision(existing),
                contents: existing.to_string(),
            });
        }
        (Some(revision), Some(existing)) if content_revision(existing) == revision => false,
        (Some(_), _) => return Err(PersistDefinitionError::Conflict),
    };
    write_agent_file(&dest, contents.as_bytes())?;
    Ok(PersistDefinitionOutcome {
        path: dest,
        contents,
        created,
    })
}

pub(crate) fn content_revision(contents: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(contents.as_bytes()))
}

pub(crate) fn persist_destination_path(
    location: AgentSaveLocation,
    cwd: &Path,
    home: Option<&Path>,
    project_trust: ProjectTrust,
    id: &str,
) -> Result<PathBuf, PersistDefinitionError> {
    if location == AgentSaveLocation::Project && !project_trust.is_trusted() {
        return Err(PersistDefinitionError::Unauthorized(
            "project agents require RHO_TRUST_PROJECT_AGENTS=1".into(),
        ));
    }
    let roots = origin_roots(location.origin(), cwd, home);
    let root = match location {
        AgentSaveLocation::Project => {
            roots.first().map(|(_, root)| root.clone()).ok_or_else(|| {
                PersistDefinitionError::Unauthorized(
                    "could not resolve a project agent directory".into(),
                )
            })?
        }
        AgentSaveLocation::AgentsHome | AgentSaveLocation::RhoHome => {
            roots.first().map(|(_, root)| root.clone()).ok_or_else(|| {
                PersistDefinitionError::Unauthorized("could not resolve home directory".into())
            })?
        }
    };
    Ok(root.join(format!("{id}.md")))
}

#[cfg(test)]
#[path = "persist_tests.rs"]
mod tests;
