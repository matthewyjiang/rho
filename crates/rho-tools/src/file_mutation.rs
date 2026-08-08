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

/// The write attempt that a fault injector may reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RewriteAttempt {
    Replacement,
    Restoration,
}

/// Test seam for deterministic failures after a file has been truncated.
pub(crate) trait RewriteFaultInjector: Send + Sync {
    fn fail_after_truncate(&self, attempt: RewriteAttempt) -> Option<std::io::Error>;
}

/// Exhaustive state after a failed locked rewrite.
pub(crate) enum RewriteFailure {
    /// The target was not changed.
    Unchanged(ToolError),
    /// The replacement changed the target, then restoration succeeded.
    Restored(ToolError),
    /// Both the replacement and restoration failed, so the target is dirty.
    Dirty {
        error: ToolError,
        restoration_error: ToolError,
    },
}

impl RewriteFailure {
    pub(crate) fn into_tool_error(self) -> ToolError {
        match self {
            Self::Unchanged(error) => error,
            Self::Restored(error) => {
                ToolError::Message(format!("{error}; original contents were restored"))
            }
            Self::Dirty {
                error,
                restoration_error,
            } => ToolError::Message(format!(
                "{error}; failed to restore original contents: {restoration_error}"
            )),
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
    rewrite_locked_file_tracked(file, display_path, original, updated, None)
        .map_err(RewriteFailure::into_tool_error)
}

/// Rewrite a locked file while preserving the final mutation state on failure.
pub(crate) fn rewrite_locked_file_tracked(
    file: &mut std::fs::File,
    display_path: &str,
    original: &str,
    updated: &str,
    fault: Option<&dyn RewriteFaultInjector>,
) -> Result<(), RewriteFailure> {
    if let Err(failure) = rewrite(
        file,
        display_path,
        updated,
        RewriteAttempt::Replacement,
        fault,
    ) {
        let error = failure.error;
        if !failure.mutated {
            return Err(RewriteFailure::Unchanged(error));
        }
        return match rewrite(
            file,
            display_path,
            original,
            RewriteAttempt::Restoration,
            fault,
        ) {
            Ok(()) => Err(RewriteFailure::Restored(error)),
            Err(restoration) => Err(RewriteFailure::Dirty {
                error,
                restoration_error: restoration.error,
            }),
        };
    }
    Ok(())
}

struct RewriteAttemptFailure {
    error: ToolError,
    mutated: bool,
}

fn rewrite(
    file: &mut std::fs::File,
    display_path: &str,
    contents: &str,
    attempt: RewriteAttempt,
    fault: Option<&dyn RewriteFaultInjector>,
) -> Result<(), RewriteAttemptFailure> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| RewriteAttemptFailure {
            error: ToolError::Message(format!("could not rewrite {display_path}: {error}")),
            mutated: false,
        })?;
    file.set_len(0).map_err(|error| RewriteAttemptFailure {
        error: ToolError::Message(format!("could not rewrite {display_path}: {error}")),
        mutated: false,
    })?;
    if let Some(error) = fault.and_then(|fault| fault.fail_after_truncate(attempt)) {
        return Err(RewriteAttemptFailure {
            error: ToolError::Message(format!("could not write {display_path}: {error}")),
            mutated: true,
        });
    }
    file.write_all(contents.as_bytes())
        .map_err(|error| RewriteAttemptFailure {
            error: ToolError::Message(format!("could not write {display_path}: {error}")),
            mutated: true,
        })?;
    file.flush().map_err(|error| RewriteAttemptFailure {
        error: ToolError::Message(format!("could not write {display_path}: {error}")),
        mutated: true,
    })
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

/// Whether an atomic create changed its target path.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicCreateTargetEffect {
    #[default]
    Unchanged,
    Installed,
}

/// Filesystem entries created while atomically creating a file.
#[derive(Debug, Default)]
pub(crate) struct AtomicCreateEffects {
    pub(crate) target: AtomicCreateTargetEffect,
    pub(crate) created_directories: Vec<PathBuf>,
    pub(crate) residual_files: Vec<PathBuf>,
}

impl AtomicCreateEffects {
    pub(crate) fn is_empty(&self) -> bool {
        self.target == AtomicCreateTargetEffect::Unchanged
            && self.created_directories.is_empty()
            && self.residual_files.is_empty()
    }
}

/// A completed atomic creation and the parent directories it owns.
#[derive(Debug)]
pub(crate) struct AtomicCreateSuccess {
    pub(crate) effects: AtomicCreateEffects,
}

/// A failed atomic creation with every filesystem effect still needing cleanup.
#[derive(Debug)]
pub(crate) struct AtomicCreateFailure {
    pub(crate) error: ToolError,
    pub(crate) effects: AtomicCreateEffects,
}

/// Installation path selected for an atomic create.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicInstallMethod {
    #[default]
    Platform,
    #[cfg(any(test, all(unix, not(target_vendor = "apple"))))]
    HardLink,
}

/// Test seam for deterministic atomic-create failures.
pub(crate) trait AtomicCreateFaultInjector: Send + Sync {
    fn fail_before_staging(&self, _display_path: &str) -> Option<std::io::Error> {
        None
    }

    fn install_method(&self, _display_path: &str) -> AtomicInstallMethod {
        AtomicInstallMethod::Platform
    }

    #[cfg(any(test, all(unix, not(target_vendor = "apple"))))]
    fn fail_staged_removal_after_hard_link(&self, _staged: &Path) -> Option<std::io::Error> {
        None
    }
}

/// Create a fully staged file without replacing a target that raced into place.
pub(crate) async fn atomic_create_file(
    path: &Path,
    display_path: &str,
    content: &str,
    permissions: Option<Permissions>,
    fault: Option<&dyn AtomicCreateFaultInjector>,
) -> Result<AtomicCreateSuccess, AtomicCreateFailure> {
    let created_directories = create_missing_parents(path, display_path).await?;
    let mut effects = AtomicCreateEffects {
        target: AtomicCreateTargetEffect::Unchanged,
        created_directories,
        residual_files: Vec::new(),
    };

    if let Some(error) = fault.and_then(|fault| fault.fail_before_staging(display_path)) {
        return Err(AtomicCreateFailure {
            error: ToolError::Message(format!("failed to stage {display_path}: {error}")),
            effects,
        });
    }

    let staged = match stage_file(path, display_path, content, permissions).await {
        Ok(staged) => staged,
        Err((error, staged)) => {
            if let Some(staged) = staged {
                match tokio::fs::remove_file(&staged).await {
                    Ok(()) => {}
                    Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => effects.residual_files.push(staged),
                }
            }
            return Err(AtomicCreateFailure { error, effects });
        }
    };

    let install_method = fault
        .map(|fault| fault.install_method(display_path))
        .unwrap_or_default();
    match install_no_replace(&staged, path, install_method, fault) {
        InstallOutcome::Installed => {
            effects.target = AtomicCreateTargetEffect::Installed;
            Ok(AtomicCreateSuccess { effects })
        }
        InstallOutcome::NotInstalled(error) => {
            match tokio::fs::remove_file(&staged).await {
                Ok(()) => {}
                Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => effects.residual_files.push(staged),
            }
            Err(AtomicCreateFailure {
                error: ToolError::Message(format!(
                    "failed to create {display_path} without replacing an existing file: {error}"
                )),
                effects,
            })
        }
        #[cfg(any(test, all(unix, not(target_vendor = "apple"))))]
        InstallOutcome::InstalledWithResidual { cleanup_error } => {
            effects.target = AtomicCreateTargetEffect::Installed;
            effects.residual_files.push(staged.clone());
            Err(AtomicCreateFailure {
                error: ToolError::Message(format!(
                    "failed to finalize {display_path}: target was installed but staged hard link {} could not be removed: {cleanup_error}",
                    staged.display()
                )),
                effects,
            })
        }
    }
}

async fn create_missing_parents(
    path: &Path,
    display_path: &str,
) -> Result<Vec<PathBuf>, AtomicCreateFailure> {
    let Some(parent) = path.parent() else {
        return Ok(Vec::new());
    };
    let mut missing = Vec::new();
    let mut cursor = parent;
    loop {
        match tokio::fs::metadata(cursor).await {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => {
                return Err(AtomicCreateFailure {
                    error: ToolError::Message(format!(
                        "failed to create parent directories for {display_path}: {} is not a directory",
                        cursor.display()
                    )),
                    effects: AtomicCreateEffects::default(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing.push(cursor),
            Err(error) => {
                return Err(AtomicCreateFailure {
                    error: ToolError::Message(format!(
                        "failed to inspect parent directories for {display_path}: {error}"
                    )),
                    effects: AtomicCreateEffects::default(),
                });
            }
        }
        let Some(next) = cursor.parent() else {
            break;
        };
        cursor = next;
    }

    let mut created = Vec::with_capacity(missing.len());
    for directory in missing.into_iter().rev() {
        match tokio::fs::create_dir(&directory).await {
            Ok(()) => created.push(directory.to_path_buf()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                match tokio::fs::metadata(&directory).await {
                    Ok(metadata) if metadata.is_dir() => {}
                    Ok(_) => {
                        return Err(parent_creation_failure(
                            display_path,
                            std::io::Error::new(
                                std::io::ErrorKind::NotADirectory,
                                format!("{} is not a directory", directory.display()),
                            ),
                            created,
                        ));
                    }
                    Err(error) => {
                        return Err(parent_creation_failure(display_path, error, created));
                    }
                }
            }
            Err(error) => return Err(parent_creation_failure(display_path, error, created)),
        }
    }
    Ok(created)
}

fn parent_creation_failure(
    display_path: &str,
    error: std::io::Error,
    created_directories: Vec<PathBuf>,
) -> AtomicCreateFailure {
    AtomicCreateFailure {
        error: ToolError::Message(format!(
            "failed to create parent directories for {display_path}: {error}"
        )),
        effects: AtomicCreateEffects {
            target: AtomicCreateTargetEffect::Unchanged,
            created_directories,
            residual_files: Vec::new(),
        },
    }
}

async fn stage_file(
    path: &Path,
    display_path: &str,
    content: &str,
    permissions: Option<Permissions>,
) -> Result<PathBuf, (ToolError, Option<PathBuf>)> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let staged = parent.join(format!(".rho-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    if let Some(permissions) = permissions.as_ref() {
        options.mode(permissions.mode());
    }
    let mut file = options.open(&staged).await.map_err(|error| {
        (
            ToolError::Message(format!("failed to stage {display_path}: {error}")),
            None,
        )
    })?;
    file.write_all(content.as_bytes()).await.map_err(|error| {
        (
            ToolError::Message(format!("failed to stage {display_path}: {error}")),
            Some(staged.clone()),
        )
    })?;
    file.flush().await.map_err(|error| {
        (
            ToolError::Message(format!("failed to stage {display_path}: {error}")),
            Some(staged.clone()),
        )
    })?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions).await.map_err(|error| {
            (
                ToolError::Message(format!(
                    "failed to preserve permissions for {display_path}: {error}"
                )),
                Some(staged.clone()),
            )
        })?;
    }
    drop(file);
    Ok(staged)
}

enum InstallOutcome {
    Installed,
    NotInstalled(std::io::Error),
    #[cfg(any(test, all(unix, not(target_vendor = "apple"))))]
    InstalledWithResidual {
        cleanup_error: std::io::Error,
    },
}

fn install_no_replace(
    staged: &Path,
    target: &Path,
    method: AtomicInstallMethod,
    fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    match method {
        AtomicInstallMethod::Platform => install_platform_no_replace(staged, target, fault),
        #[cfg(any(test, all(unix, not(target_vendor = "apple"))))]
        AtomicInstallMethod::HardLink => install_with_hard_link(staged, target, fault),
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn install_platform_no_replace(
    staged: &Path,
    target: &Path,
    fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    fn c_path(path: &Path) -> std::io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    }

    let staged_c = match c_path(staged) {
        Ok(path) => path,
        Err(error) => return InstallOutcome::NotInstalled(error),
    };
    let target_c = match c_path(target) {
        Ok(path) => path,
        Err(error) => return InstallOutcome::NotInstalled(error),
    };
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
        return InstallOutcome::Installed;
    }
    let error = std::io::Error::last_os_error();
    if !matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS) | Some(libc::EINVAL)
    ) {
        return InstallOutcome::NotInstalled(error);
    }
    install_no_replace(staged, target, AtomicInstallMethod::HardLink, fault)
}

#[cfg(target_vendor = "apple")]
fn install_platform_no_replace(
    staged: &Path,
    target: &Path,
    _fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let staged = match CString::new(staged.as_os_str().as_bytes()) {
        Ok(staged) => staged,
        Err(_) => {
            return InstallOutcome::NotInstalled(std::io::Error::from(
                std::io::ErrorKind::InvalidInput,
            ));
        }
    };
    let target = match CString::new(target.as_os_str().as_bytes()) {
        Ok(target) => target,
        Err(_) => {
            return InstallOutcome::NotInstalled(std::io::Error::from(
                std::io::ErrorKind::InvalidInput,
            ));
        }
    };
    // SAFETY: Both paths are NUL-terminated and valid for the duration of the call.
    let result = unsafe { libc::renamex_np(staged.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        InstallOutcome::Installed
    } else {
        InstallOutcome::NotInstalled(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn install_platform_no_replace(
    staged: &Path,
    target: &Path,
    _fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
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
        InstallOutcome::NotInstalled(std::io::Error::last_os_error())
    } else {
        InstallOutcome::Installed
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple"))
))]
fn install_platform_no_replace(
    staged: &Path,
    target: &Path,
    fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    install_no_replace(staged, target, AtomicInstallMethod::HardLink, fault)
}

#[cfg(any(test, all(unix, not(target_vendor = "apple"))))]
fn install_with_hard_link(
    staged: &Path,
    target: &Path,
    fault: Option<&dyn AtomicCreateFaultInjector>,
) -> InstallOutcome {
    if let Err(error) = std::fs::hard_link(staged, target) {
        return InstallOutcome::NotInstalled(error);
    }
    let removal = fault
        .and_then(|fault| fault.fail_staged_removal_after_hard_link(staged))
        .map_or_else(|| std::fs::remove_file(staged), Err);
    match removal {
        Ok(()) => InstallOutcome::Installed,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => InstallOutcome::Installed,
        Err(cleanup_error) => InstallOutcome::InstalledWithResidual { cleanup_error },
    }
}
