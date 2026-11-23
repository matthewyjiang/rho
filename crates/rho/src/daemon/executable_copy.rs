//! Copies of the rho executable for a daemon to run instead of the
//! installed file, so no process runs from the installation directory. Scoop
//! refuses to update an app while a process runs from the app's directory,
//! so without one the daemon would block every `scoop update`.
//!
//! The copies live in one directory, which holds nothing else. Each copy's
//! name comes from the length and modification time of the file it copies,
//! so starting from the same install again reuses its copy, and an update
//! gets a new one. Each time a copy is staged, the others are removed. A copy
//! that a daemon still runs may resist removal, as Windows refuses to delete
//! a running program's file; a later start removes it.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use super::platform;

/// Returns the path of a copy of `executable` in `dir`, copying it there
/// first unless a copy of the file as it is now is already there, and
/// removes every other file in `dir`.
pub(super) fn stage(executable: &Path, dir: &Path) -> io::Result<PathBuf> {
    let name = copy_name(executable)?;
    let copy = dir.join(&name);
    platform::create_private_dir(dir)?;
    if !copy.try_exists()? {
        // Copied under another name first, so `copy` never names a partial
        // file that a later start would trust.
        let partial = dir.join(format!(".partial-{}", std::process::id()));
        fs::copy(executable, &partial)
            .and_then(|_| fs::rename(&partial, &copy))
            .inspect_err(|_| {
                let _ = fs::remove_file(&partial);
            })?;
    }
    remove_others(dir, &name);
    Ok(copy)
}

/// `rho-<length>-<modified ns>`, keeping `executable`'s extension.
fn copy_name(executable: &Path) -> io::Result<OsString> {
    let metadata = fs::metadata(executable)?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut name = OsString::from(format!("rho-{}-{modified}", metadata.len()));
    if let Some(extension) = executable.extension() {
        name.push(".");
        name.push(extension);
    }
    Ok(name)
}

fn remove_others(dir: &Path, keep: &OsString) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten().filter(|entry| entry.file_name() != *keep) {
        if let Err(error) = fs::remove_file(entry.path()) {
            tracing::debug!(path = %entry.path().display(), %error, "kept an earlier rho executable copy");
        }
    }
}

#[cfg(test)]
#[path = "executable_copy_tests.rs"]
mod tests;
