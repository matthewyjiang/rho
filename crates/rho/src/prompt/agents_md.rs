//! Transactional updates to standing instructions consumed by prompt discovery.

use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::Path,
};

use anyhow::{ensure, Result};

use crate::config_writer::{self, edit_lock::acquire_edit_lock};

/// Append one instruction under a stable cross-process lock covering the entire
/// read-modify-replace transaction, including creation of a missing file.
/// Contention fails without modifying the file; the caller can retry.
pub(crate) fn append_instruction(path: &Path, text: &str) -> Result<()> {
    let _lock = acquire_edit_lock(path)?;
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
    if existing.is_some() {
        config_writer::replace_regular_file_atomically(path, contents.as_bytes())?;
    } else {
        config_writer::write_atomically(path, &contents)?;
    }
    Ok(())
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
