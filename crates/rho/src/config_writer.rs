use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// Atomically replace `path` with `contents` via a unique temp file.
///
/// On Windows, an existing destination is replaced with `ReplaceFileW` so
/// repeated updates do not fail the way a plain rename-over-existing can.
pub(crate) fn write_bytes_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    write_bytes_atomically_with_permissions(path, contents, None)
}

/// Atomically replaces a regular file while retaining its current permissions.
///
/// Unlike [`write_bytes_atomically`], this rejects symlink destinations so a
/// caller cannot follow a link while preserving source-file metadata.
pub(crate) fn replace_regular_file_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "destination is not a regular file",
        ));
    }
    write_bytes_atomically_with_permissions(path, contents, Some(metadata.permissions()))
}

fn write_bytes_atomically_with_permissions(
    path: &Path,
    contents: &[u8],
    permissions: Option<fs::Permissions>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = unique_temp_path(path);
    let result = (|| {
        let mut temp_file = create_temp_file(&temp_path)?;
        set_private_file_permissions(&temp_file)?;
        if let Some(permissions) = permissions {
            temp_file.set_permissions(permissions)?;
        }
        temp_file.write_all(contents)?;
        temp_file.sync_all()?;
        drop(temp_file);
        replace_file(&temp_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

pub(super) fn write_atomically(path: &Path, contents: &str) -> anyhow::Result<()> {
    write_bytes_atomically(path, contents.as_bytes()).map_err(Into::into)
}

fn unique_temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    path.with_file_name(format!(
        ".{file_name}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ))
}

fn create_temp_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(not(windows))]
fn replace_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(temp_path, path)
}

#[cfg(windows)]
fn replace_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    // Call ReplaceFileW directly. Checking path.exists() first is a TOCTOU race
    // against another writer creating or deleting the destination.
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    let replaced = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replacement = temp_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result != 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) {
        return fs::rename(temp_path, path);
    }
    Err(error)
}

#[cfg(unix)]
fn set_private_file_permissions(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_file: &File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "config_writer_tests.rs"]
mod tests;
