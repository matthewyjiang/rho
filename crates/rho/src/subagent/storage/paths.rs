//! Trusted on-disk layout for nested and global delegated runs.

use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use super::super::{create_private_directory, secure_directory};

/// Validated `sessions/<workspace>/<unit>/subagents` path.
#[derive(Clone, Debug)]
pub(super) struct SessionSubagentsDir {
    path: PathBuf,
}

impl SessionSubagentsDir {
    pub(super) fn parse(rho_root: &Path, path: &Path) -> anyhow::Result<Self> {
        let sessions_root = rho_root.join("sessions");
        let relative = path.strip_prefix(&sessions_root).map_err(|_| {
            anyhow::anyhow!(
                "session delegated run directory is outside {}",
                sessions_root.display()
            )
        })?;
        let components = relative.components().collect::<Vec<_>>();
        let [workspace, session, subagents] = components.as_slice() else {
            anyhow::bail!(
                "invalid session delegated run directory: {}",
                path.display()
            );
        };
        anyhow::ensure!(
            matches!(subagents, Component::Normal(name) if *name == "subagents"),
            "invalid session delegated run directory: {}",
            path.display()
        );
        anyhow::ensure!(
            matches!(workspace, Component::Normal(_)) && matches!(session, Component::Normal(_)),
            "invalid session delegated run directory: {}",
            path.display()
        );

        let workspace_path = sessions_root.join(workspace.as_os_str());
        let session_path = workspace_path.join(session.as_os_str());
        for ancestor in [&sessions_root, &workspace_path, &session_path] {
            anyhow::ensure!(
                is_trusted_directory(ancestor),
                "{} is not a trusted session directory",
                ancestor.display()
            );
        }
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    pub(super) fn ensure_ready(&self) -> anyhow::Result<()> {
        // Concurrent first reservations race on this create; AlreadyExists is
        // success. `secure_directory` re-validates type and mode either way.
        match create_private_directory(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        secure_directory(&self.path)?;
        Ok(())
    }

    pub(super) fn run_directory(&self, id: &str) -> PathBuf {
        self.path.join(id)
    }
}

pub(crate) fn is_trusted_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

pub(super) fn validate_run_directory(rho_root: &Path, id: &str, path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        path.file_name().and_then(|name| name.to_str()) == Some(id),
        "delegated run index contains an invalid path for '{id}': {}",
        path.display()
    );

    let global_root = rho_root.join("subagents");
    if path.parent() == Some(global_root.as_path()) && is_trusted_directory(&global_root) {
        return Ok(());
    }

    let parent = path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "delegated run index contains an invalid path for '{id}': {}",
            path.display()
        )
    })?;
    SessionSubagentsDir::parse(rho_root, parent).map_err(|_| {
        anyhow::anyhow!(
            "delegated run index contains an invalid path for '{id}': {}",
            path.display()
        )
    })?;
    Ok(())
}

pub(super) fn scan_session_directories(rho_root: &Path, id: &str) -> anyhow::Result<Vec<PathBuf>> {
    let sessions_root = rho_root.join("sessions");
    let mut matches = Vec::new();
    if !is_trusted_directory(&sessions_root) {
        return Ok(matches);
    }
    for workspace in fs::read_dir(sessions_root)? {
        let workspace = workspace?.path();
        if !is_trusted_directory(&workspace) {
            continue;
        }
        for session in fs::read_dir(workspace)? {
            let session = session?.path();
            if !is_trusted_directory(&session) {
                continue;
            }
            let subagents = session.join("subagents");
            let Ok(session_subagents) = SessionSubagentsDir::parse(rho_root, &subagents) else {
                continue;
            };
            if !is_trusted_directory(&session_subagents.path) {
                continue;
            }
            let candidate = session_subagents.run_directory(id);
            if is_trusted_directory(&candidate) {
                matches.push(candidate);
            }
        }
    }
    matches.sort();
    Ok(matches)
}

pub(super) fn prepare_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    secure_directory(path)
}
