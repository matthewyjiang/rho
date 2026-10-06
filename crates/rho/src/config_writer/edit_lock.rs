//! Stable sidecar locks for read-modify-replace file edits.

use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

pub(crate) fn edit_lock_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        ".{}.rho-edit.lock",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agent")
    ))
}

/// Lock `path` through a sidecar next to it. See [`acquire_lock_file`].
pub(crate) fn acquire_edit_lock(path: &Path) -> io::Result<EditFileLock> {
    acquire_lock_file(&edit_lock_path(path))
}

/// Take an exclusive lock on `lock_path` without waiting. A busy writer is
/// reported to the caller. Drop unlocks but never unlinks the lock file, so
/// openers keep one lock identity.
pub(crate) fn acquire_lock_file(lock_path: &Path) -> io::Result<EditFileLock> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(lock_path).map_err(|error| {
        io::Error::new(error.kind(), format!("could not open edit lock: {error}"))
    })?;
    fs2::FileExt::try_lock_exclusive(&file).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not lock file for editing: {error}"),
        )
    })?;
    Ok(EditFileLock { file })
}

pub(crate) struct EditFileLock {
    file: File,
}

impl Drop for EditFileLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}
