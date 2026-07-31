use std::{fs, path::Path};

use sha2::{Digest as _, Sha256};

use crate::workflow::{ArtifactObservation, ArtifactRef, Digest};

use super::RuntimeError;

pub(super) fn ensure_private_directory(path: &Path) -> Result<(), RuntimeError> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(RuntimeError::UnsafeArtifact(path.to_path_buf()));
        }
    } else {
        fs::create_dir_all(path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(super) fn write_artifact(
    run_directory: &Path,
    path: &Path,
    bytes: &[u8],
) -> Result<ArtifactRef, RuntimeError> {
    write_artifact_with_observation(
        run_directory,
        path,
        bytes,
        ArtifactObservation::Complete {
            observed_bytes: bytes.len() as u64,
        },
    )
}

pub(super) fn write_artifact_with_observation(
    run_directory: &Path,
    path: &Path,
    bytes: &[u8],
    observed: ArtifactObservation,
) -> Result<ArtifactRef, RuntimeError> {
    let relative = path
        .strip_prefix(run_directory)
        .map_err(|_| RuntimeError::UnsafeArtifact(path.to_path_buf()))?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(RuntimeError::UnsafeArtifact(path.to_path_buf()));
    }
    crate::workflow::write_file_beneath(run_directory, relative, bytes)?;
    Ok(ArtifactRef {
        relative_path: relative.to_string_lossy().replace('\\', "/"),
        retained_bytes: bytes.len() as u64,
        observed,
        digest: Digest(format!("sha256:{:x}", Sha256::digest(bytes))),
    })
}

pub(super) fn write_json(
    run_directory: &Path,
    path: &Path,
    value: &impl serde::Serialize,
) -> Result<ArtifactRef, RuntimeError> {
    write_artifact(run_directory, path, &serde_json::to_vec_pretty(value)?)
}
