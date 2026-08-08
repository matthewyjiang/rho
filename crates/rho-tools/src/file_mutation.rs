//! Shared mechanics and result shape for workspace file mutations.

use std::{
    fs::{OpenOptions, Permissions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use tokio::io::AsyncWriteExt;

use crate::tool::ToolError;

/// Model-facing content plus UI metadata for a completed file mutation.
#[derive(Debug)]
pub(crate) struct FileMutationOutcome {
    pub content: String,
    /// Display paths touched by the mutation, in document order.
    pub display_paths: Vec<String>,
    /// Unified diff for UI cards (not repeated in model-facing content).
    pub diff: String,
}

/// Open and exclusively lock one existing file for a bounded rewrite.
pub(crate) fn lock_for_rewrite(
    path: &Path,
    display_path: &str,
    context: &str,
) -> Result<std::fs::File, ToolError> {
    const LOCK_TIMEOUT: Duration = Duration::from_secs(1);
    const INITIAL_BACKOFF: Duration = Duration::from_millis(2);
    const MAX_BACKOFF: Duration = Duration::from_millis(50);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            ToolError::Message(format!("could not open {display_path}{context}: {error}"))
        })?;

    let deadline = Instant::now() + LOCK_TIMEOUT;
    let mut backoff = INITIAL_BACKOFF;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(ToolError::Message(format!(
                        "could not lock {display_path}{context}: timed out after {}ms",
                        LOCK_TIMEOUT.as_millis()
                    )));
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(ToolError::Message(format!(
                    "could not lock {display_path}{context}: {error}"
                )));
            }
        }
    }
}

/// Rewrite a locked file and restore its original bytes if the write fails.
pub(crate) fn rewrite_locked_file(
    file: &mut std::fs::File,
    display_path: &str,
    original: &str,
    updated: &str,
) -> Result<(), ToolError> {
    if let Err(error) = rewrite(file, display_path, updated) {
        return match rewrite(file, display_path, original) {
            Ok(()) => Err(ToolError::Message(format!(
                "{error}; original contents were restored"
            ))),
            Err(rollback_error) => Err(ToolError::Message(format!(
                "{error}; failed to restore original contents: {rollback_error}"
            ))),
        };
    }
    Ok(())
}

fn rewrite(file: &mut std::fs::File, display_path: &str, contents: &str) -> Result<(), ToolError> {
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        ToolError::Message(format!("could not rewrite {display_path}: {error}"))
    })?;
    file.set_len(0).map_err(|error| {
        ToolError::Message(format!("could not rewrite {display_path}: {error}"))
    })?;
    file.write_all(contents.as_bytes())
        .map_err(|error| ToolError::Message(format!("could not write {display_path}: {error}")))?;
    file.flush()
        .map_err(|error| ToolError::Message(format!("could not write {display_path}: {error}")))
}

/// Normalize CRLF and bare CR line endings to LF.
pub(crate) fn normalize_newlines(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(ch);
        }
    }
    normalized
}

/// Choose the predominant line ending in existing text.
pub(crate) fn preferred_line_ending(value: &str) -> &'static str {
    let crlf = value.matches("\r\n").count();
    let lf = value.bytes().filter(|byte| *byte == b'\n').count() - crlf;
    let bytes = value.as_bytes();
    let cr = bytes
        .iter()
        .enumerate()
        .filter(|(index, byte)| **byte == b'\r' && bytes.get(index + 1) != Some(&b'\n'))
        .count();
    if cr > crlf && cr > lf {
        "\r"
    } else if crlf > lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Create a fully staged file without replacing a target that raced into place.
pub(crate) async fn atomic_create_file(
    path: &Path,
    display_path: &str,
    content: &str,
    permissions: Option<Permissions>,
) -> Result<(), ToolError> {
    let staged = stage_file(path, display_path, content, permissions).await?;
    let staged_for_install = staged.clone();
    let target = path.to_path_buf();
    let result =
        tokio::task::spawn_blocking(move || install_no_replace(&staged_for_install, &target))
            .await
            .map_err(|error| ToolError::Message(format!("file install task failed: {error}")))?
            .map_err(|error| {
                ToolError::Message(format!(
                    "failed to create {display_path} without replacing an existing file: {error}"
                ))
            });
    if result.is_err() {
        let _ = tokio::fs::remove_file(&staged).await;
    }
    result
}

async fn stage_file(
    path: &Path,
    display_path: &str,
    content: &str,
    permissions: Option<Permissions>,
) -> Result<PathBuf, ToolError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            ToolError::Message(format!(
                "failed to create parent directories for {display_path}: {error}"
            ))
        })?;
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let staged = parent.join(format!(".rho-{}.tmp", uuid::Uuid::new_v4()));
    let result = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        if let Some(permissions) = permissions.as_ref() {
            options.mode(permissions.mode());
        }
        let mut file = options.open(&staged).await.map_err(|error| {
            ToolError::Message(format!("failed to stage {display_path}: {error}"))
        })?;
        file.write_all(content.as_bytes()).await.map_err(|error| {
            ToolError::Message(format!("failed to stage {display_path}: {error}"))
        })?;
        file.flush().await.map_err(|error| {
            ToolError::Message(format!("failed to stage {display_path}: {error}"))
        })?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions).await.map_err(|error| {
                ToolError::Message(format!(
                    "failed to preserve permissions for {display_path}: {error}"
                ))
            })?;
        }
        drop(file);
        Ok(staged.clone())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&staged).await;
    }
    result
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn install_no_replace(staged: &Path, target: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    fn c_path(path: &Path) -> std::io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    }

    let staged_c = c_path(staged)?;
    let target_c = c_path(target)?;
    // SAFETY: Both paths are NUL-terminated and valid for the duration of the call.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            staged_c.as_ptr(),
            libc::AT_FDCWD,
            target_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if !matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS) | Some(libc::EINVAL)
    ) {
        return Err(error);
    }
    install_with_hard_link(staged, target)
}

#[cfg(target_vendor = "apple")]
fn install_no_replace(staged: &Path, target: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let staged = CString::new(staged.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: Both paths are NUL-terminated and valid for the duration of the call.
    let result = unsafe { libc::renamex_np(staged.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn install_no_replace(staged: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    let staged: Vec<u16> = staged
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: Both paths are NUL-terminated and valid for the duration of the call.
    let result = unsafe { MoveFileExW(staged.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple"))
))]
fn install_no_replace(staged: &Path, target: &Path) -> std::io::Result<()> {
    install_with_hard_link(staged, target)
}

#[cfg(unix)]
fn install_with_hard_link(staged: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::hard_link(staged, target)?;
    let _ = std::fs::remove_file(staged);
    Ok(())
}
