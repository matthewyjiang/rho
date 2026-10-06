//! Transactional updates to standing instructions consumed by prompt discovery.

use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Result};

use sha2::{Digest, Sha256};

use crate::config_writer::{self, edit_lock::acquire_lock_file};

/// Append one instruction under a stable cross-process lock covering the entire
/// read-modify-replace transaction, including creation of a missing file.
/// Contention fails without modifying the file; the caller can retry.
///
/// The lock lives under `lock_dir` (Rho's data directory), not beside the
/// file: a sidecar in a repository root would show up in `git status`.
pub(crate) fn append_instruction(path: &Path, lock_dir: &Path, text: &str) -> Result<()> {
    let _lock = acquire_lock_file(&instruction_lock_path(path, lock_dir)).map_err(|error| {
        if error.kind() == ErrorKind::WouldBlock {
            anyhow::anyhow!("another session is saving instructions; retry")
        } else {
            error.into()
        }
    })?;
    let existing = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(metadata.is_file(), "destination is not a regular file");
            ensure!(
                !metadata.permissions().readonly(),
                "destination is read-only"
            );
            // Match model prompt editing: a writable parent must not bypass
            // the destination's own write permissions or ACLs.
            OpenOptions::new().write(true).open(path)?;
            Some(fs::read_to_string(path)?)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let contents = appended_contents(existing.as_deref(), text);
    if existing.is_none() {
        // AGENTS.md is shared project text, not a secret: create it with the
        // process umask like any editor would, then fill it atomically.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new().write(true).create_new(true).open(path)?;
    }
    config_writer::replace_regular_file_atomically(path, contents.as_bytes())?;
    Ok(())
}

/// One lock per destination. The parent is canonicalized so symlinked
/// spellings of the same directory share a lock.
fn instruction_lock_path(path: &Path, lock_dir: &Path) -> PathBuf {
    let canonical = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(path.file_name())
        .map_or_else(|| path.to_path_buf(), |(parent, name)| parent.join(name));
    let key = hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()));
    lock_dir.join(format!("agents-md-{key}.lock"))
}

/// Preserve existing instructions and their newline style, with one bullet separator.
fn appended_contents(existing: Option<&str>, text: &str) -> String {
    let existing = existing.unwrap_or_default();
    let newline = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let existing = existing.trim_end_matches(['\r', '\n']);
    let text = text.trim();
    if existing.is_empty() {
        format!("- {text}{newline}")
    } else {
        format!("{existing}{newline}- {text}{newline}")
    }
}

#[cfg(test)]
#[path = "agents_md_tests.rs"]
mod tests;
