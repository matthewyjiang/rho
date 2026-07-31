use std::{fs, path::Path};

use sha2::{Digest as _, Sha256};

use crate::workflow::{ArtifactRef, Digest};

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
    let parent = path
        .parent()
        .ok_or_else(|| RuntimeError::UnsafeArtifact(path.to_path_buf()))?;
    ensure_private_directory(parent)?;
    let canonical_run = run_directory.canonicalize()?;
    let canonical_parent = parent.canonicalize()?;
    if !canonical_parent.starts_with(&canonical_run) || path.exists() && path.is_symlink() {
        return Err(RuntimeError::UnsafeArtifact(path.to_path_buf()));
    }
    crate::config_writer::write_bytes_atomically(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    let relative = path
        .strip_prefix(&canonical_run)
        .or_else(|_| path.strip_prefix(run_directory))
        .map_err(|_| RuntimeError::UnsafeArtifact(path.to_path_buf()))?;
    Ok(ArtifactRef {
        relative_path: relative.to_string_lossy().replace('\\', "/"),
        bytes: bytes.len() as u64,
        digest: Digest(format!("sha256:{:x}", Sha256::digest(bytes))),
    })
}

pub(super) fn write_json(
    run_directory: &Path,
    path: &Path,
    value: &impl serde::Serialize,
) -> Result<(), RuntimeError> {
    write_artifact(run_directory, path, &serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
