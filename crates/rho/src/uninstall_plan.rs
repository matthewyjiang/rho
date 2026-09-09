//! Conservative deletion policy and an ordered, identity-checked removal plan.
use super::Paths;
use crate::installation::{self, InstallHint, InstallationEvidence, ManagedInstallation};
use anyhow::{bail, Context, Result};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Installation {
    Script,
    Managed(ManagedInstallation),
    Unknown,
}

pub(super) struct Removal {
    pub path: PathBuf,
    pub kind: TargetKind,
    pub identity: same_file::Handle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TargetKind {
    Executable,
    DataDirectory,
}

pub(super) struct RemovalPlan {
    pub installation: Installation,
    /// Execution order, also used for preview. Keep the executable for retry if purge fails.
    pub removals: Vec<Removal>,
}

pub(super) fn installation(paths: &Paths) -> Installation {
    installation_with_evidence(paths, installation::detect(&paths.executable))
}

pub(super) fn installation_with_evidence(
    paths: &Paths,
    evidence: InstallationEvidence,
) -> Installation {
    // Package evidence always wins over environment hints. Neither a script
    // hint nor update's permissive fallback is permission to delete a binary.
    if let Some(managed) = evidence.managed {
        return Installation::Managed(managed);
    }
    if evidence.cargo_metadata || evidence.pacman_owned {
        return Installation::Unknown;
    }
    match paths
        .install_method
        .as_deref()
        .map(installation::install_hint)
    {
        Some(InstallHint::Cargo) => {
            return Installation::Managed(ManagedInstallation::Cargo {
                root: installation::cargo_root_from_bin_path(&paths.executable),
            })
        }
        Some(InstallHint::Pacman) => return Installation::Managed(ManagedInstallation::Pacman),
        Some(InstallHint::Scoop(scope)) => {
            return Installation::Managed(ManagedInstallation::Scoop(scope))
        }
        Some(InstallHint::Unknown)
            if paths
                .install_method
                .as_deref()
                .is_some_and(|hint| !hint.trim().is_empty()) =>
        {
            return Installation::Unknown
        }
        Some(InstallHint::Unknown | InstallHint::Script) | None => {}
    }
    if cfg!(unix) && paths.executable == paths.home.join(".local/bin/rho") {
        Installation::Script
    } else {
        Installation::Unknown
    }
}

pub(super) fn plan(paths: &Paths, purge: bool) -> Result<RemovalPlan> {
    validate_path(&paths.home)?;
    if !paths.home.is_dir() {
        bail!(
            "refusing unsafe home directory {:?}: not a directory",
            paths.home
        );
    }
    let installation = installation(paths);
    let mut removals = Vec::new();
    if purge {
        let data = paths.home.join(".rho");
        if let Some(target) = removal(data.clone(), TargetKind::DataDirectory)? {
            if paths.executable.starts_with(&data) {
                bail!("refusing to purge {data:?}: running executable is inside it; move or uninstall that executable first");
            }
            removals.push(target);
        }
    }
    if installation == Installation::Script {
        if let Some(target) = removal(paths.executable.clone(), TargetKind::Executable)? {
            removals.push(target);
        }
    }
    Ok(RemovalPlan {
        installation,
        removals,
    })
}

// Refuse roots, relative paths, traversal, and linked ancestors rather than
// turning an ambiguous path into authority to delete a different directory.
pub(super) fn validate_path(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.parent().is_none() {
        bail!("refusing unsafe uninstall path {path:?}: expected an absolute non-root path");
    }
    for component in path.components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            bail!("refusing unsafe uninstall path {path:?}: traversal component");
        }
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if is_link(&metadata) {
                    bail!("refusing unsafe uninstall path {path:?}: linked ancestor {ancestor:?}");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("could not inspect {ancestor:?}"))
            }
        }
    }
    Ok(())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions and other reparse points also redirect filesystem access.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(super) fn removal(path: PathBuf, kind: TargetKind) -> Result<Option<Removal>> {
    validate_path(&path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("could not inspect {path:?}")),
    };
    let expected_type = match kind {
        TargetKind::Executable => metadata.is_file(),
        TargetKind::DataDirectory => metadata.is_dir(),
    };
    if !expected_type {
        bail!("refusing unexpected file type at {path:?}");
    }
    let identity = same_file::Handle::from_path(&path)?;
    Ok(Some(Removal {
        path,
        kind,
        identity,
    }))
}
