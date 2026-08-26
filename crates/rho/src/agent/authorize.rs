//! Authorizes agent definition files against discovery roots.
//!
//! Shared by the TUI editor (existing files) and host-owned persist (create or
//! replace). Roots match catalog discovery: `~/.agents/agents`, `~/.rho/agents`,
//! and trusted `<project>/.agents/agents`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;

use super::AgentOrigin;

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum AuthorizeAgentPathError {
    #[error("agent source is outside an editable agent directory")]
    OutsideRoot,
    #[error("editable agent root is outside its source base")]
    RootOutsideBase,
    #[error("editable agent directory must not contain symlinks")]
    SymlinkInRoot,
    #[error("editable agent directory must not be a symlink")]
    RootIsSymlink,
    #[error("editable agent source must be a regular file")]
    NotRegularFile,
    #[error("could not inspect {path}: {message}")]
    Inspect { path: String, message: String },
}

#[derive(Clone, Copy)]
enum DestinationPresence {
    MustExist,
    MayCreate,
}

/// Authorizes an existing agent file for in-place editing.
pub(crate) fn authorize_existing_agent_file(
    origin: AgentOrigin,
    path: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<PathBuf, AuthorizeAgentPathError> {
    authorize_agent_path(origin, path, cwd, home, DestinationPresence::MustExist)
}

/// Authorizes a destination that persist may create or replace.
pub(crate) fn authorize_agent_destination(
    origin: AgentOrigin,
    path: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<PathBuf, AuthorizeAgentPathError> {
    authorize_agent_path(origin, path, cwd, home, DestinationPresence::MayCreate)
}

fn authorize_agent_path(
    origin: AgentOrigin,
    path: &Path,
    cwd: &Path,
    home: Option<&Path>,
    presence: DestinationPresence,
) -> Result<PathBuf, AuthorizeAgentPathError> {
    let roots = origin_roots(origin, cwd, home);
    let (base, root) = roots
        .into_iter()
        .find(|(_, root)| path.parent() == Some(root.as_path()))
        .ok_or(AuthorizeAgentPathError::OutsideRoot)?;
    inspect_root_chain(&base, &root, presence)?;
    inspect_destination_file(path, presence)?;
    Ok(root)
}

pub(crate) fn origin_roots(
    origin: AgentOrigin,
    cwd: &Path,
    home: Option<&Path>,
) -> Vec<(PathBuf, PathBuf)> {
    match origin {
        AgentOrigin::AgentsHome => home
            .map(|home| {
                let [agents_home, _] = crate::paths::user_agent_dirs(home);
                vec![(home.to_path_buf(), agents_home)]
            })
            .unwrap_or_default(),
        AgentOrigin::RhoHome => home
            .map(|home| {
                let [_, rho_home] = crate::paths::user_agent_dirs(home);
                vec![(home.to_path_buf(), rho_home)]
            })
            .unwrap_or_default(),
        AgentOrigin::Project => crate::workspace::project_ancestor_dirs(cwd)
            .into_iter()
            .map(|base| {
                let root = base.join(".agents/agents");
                (base, root)
            })
            .collect(),
        AgentOrigin::Internal | AgentOrigin::BuiltIn | AgentOrigin::Workflow => Vec::new(),
    }
}

fn inspect_root_chain(
    base: &Path,
    root: &Path,
    presence: DestinationPresence,
) -> Result<(), AuthorizeAgentPathError> {
    let relative = root
        .strip_prefix(base)
        .map_err(|_| AuthorizeAgentPathError::RootOutsideBase)?;
    let mut component_path = base.to_path_buf();
    for component in relative.components() {
        component_path.push(component);
        match fs::symlink_metadata(&component_path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(if component_path == root {
                        AuthorizeAgentPathError::RootIsSymlink
                    } else {
                        AuthorizeAgentPathError::SymlinkInRoot
                    });
                }
                if component_path == root && !metadata.is_dir() {
                    return Err(AuthorizeAgentPathError::RootIsSymlink);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if matches!(presence, DestinationPresence::MustExist) {
                    return Err(inspect_error(&component_path, error));
                }
                return Ok(());
            }
            Err(error) => return Err(inspect_error(&component_path, error)),
        }
    }
    Ok(())
}

fn inspect_destination_file(
    path: &Path,
    presence: DestinationPresence,
) -> Result<(), AuthorizeAgentPathError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                Err(AuthorizeAgentPathError::NotRegularFile)
            } else {
                Ok(())
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => match presence {
            DestinationPresence::MayCreate => Ok(()),
            DestinationPresence::MustExist => Err(inspect_error(path, error)),
        },
        Err(error) => Err(inspect_error(path, error)),
    }
}

fn inspect_error(path: &Path, error: io::Error) -> AuthorizeAgentPathError {
    AuthorizeAgentPathError::Inspect {
        path: crate::paths::display(path),
        message: error.to_string(),
    }
}
