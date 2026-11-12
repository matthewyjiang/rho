//! Private directories and files, and which file a path names.

use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Why the daemon's files cannot be used.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DaemonPathError {
    #[error(transparent)]
    SocketPathTooLong(#[from] rho_protocol::SocketPathTooLong),
    #[error("rho daemon directory {path} is not a directory; remove it and retry")]
    NotADirectory { path: PathBuf },
    #[error(
        "rho daemon directory {path} is owned by uid {owner}, not uid {uid}; remove it and retry"
    )]
    WrongOwner { path: PathBuf, owner: u32, uid: u32 },
    #[error(
        "rho daemon directory {path} has mode {mode:04o} but must not be accessible to group or others; run `chmod 700 {path}`"
    )]
    InsecureMode { path: PathBuf, mode: u32 },
    #[error("could not prepare rho daemon directory {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("could not read {path}: {message}")]
    Unreadable { path: PathBuf, message: String },
    #[error(
        "{path} is not a socket; run `rho daemon stop`, remove it, and let the next client start the daemon"
    )]
    NotASocket { path: PathBuf },
    #[error(
        "rho daemon socket {path} is owned by uid {owner}, not uid {uid}; remove it and let the next client start the daemon"
    )]
    SocketWrongOwner { path: PathBuf, owner: u32, uid: u32 },
    #[error(
        "rho daemon socket {path} has mode {mode:04o} but must not be accessible to group or others; run `chmod 600 {path}`"
    )]
    InsecureSocketMode { path: PathBuf, mode: u32 },
}

impl DaemonPathError {
    /// Whether the files could not be checked at all, rather than checked
    /// and found open to someone else or unusable.
    pub(crate) fn is_unreadable(&self) -> bool {
        match self {
            Self::Io { .. } | Self::Unreadable { .. } => true,
            Self::SocketPathTooLong(_)
            | Self::NotADirectory { .. }
            | Self::WrongOwner { .. }
            | Self::InsecureMode { .. }
            | Self::NotASocket { .. }
            | Self::SocketWrongOwner { .. }
            | Self::InsecureSocketMode { .. } => false,
        }
    }
}

/// Creates `path` and its missing parents with mode 0700. An existing
/// directory is left as it is.
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

/// Creates `path` with mode 0700, or verifies an existing one is a real
/// directory owned by this user and closed to everyone else.
pub(in crate::daemon) fn prepare_private_dir(path: &Path) -> Result<(), DaemonPathError> {
    let io_error = |error: io::Error| DaemonPathError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    };
    match create_private_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error(error)),
    }
    // Not following symlinks: a link could point anywhere.
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    verify_private_dir(path, &metadata)
}

/// Verifies that `metadata`, read from `path` without following symlinks, is
/// of a real directory owned by this user and closed to everyone else.
pub(super) fn verify_private_dir(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), DaemonPathError> {
    if !metadata.is_dir() {
        return Err(DaemonPathError::NotADirectory {
            path: path.to_path_buf(),
        });
    }
    let uid = current_uid();
    if metadata.uid() != uid {
        return Err(DaemonPathError::WrongOwner {
            path: path.to_path_buf(),
            owner: metadata.uid(),
            uid,
        });
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(DaemonPathError::InsecureMode {
            path: path.to_path_buf(),
            mode,
        });
    }
    Ok(())
}

/// Opens the lock file `path` for reading and writing, creating it with
/// mode 0600, and never through a symlink.
pub(in crate::daemon) fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

/// Opens the log file `path` for appending, creating it with mode 0600.
pub(in crate::daemon) fn open_log_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

/// Flushes the directory `path`, so the files created, renamed, or removed
/// in it stay that way across a crash.
pub(crate) fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// Opens `path` for writing, emptied, creating it with mode 0600.
pub(crate) fn create_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

/// The effective uid of this process.
pub(super) fn current_uid() -> u32 {
    // SAFETY: `geteuid` takes no pointers, has no preconditions, and cannot fail.
    unsafe { libc::geteuid() }
}

/// Which file a path names: equal identities are the same file, unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    modified_secs: i64,
    modified_nanos: i64,
}

impl FileIdentity {
    pub(crate) fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.size(),
            modified_secs: metadata.mtime(),
            modified_nanos: metadata.mtime_nsec(),
        }
    }
}

/// Whether an installer can put a new executable at the daemon's path while
/// the daemon runs. It can: a rename over the path leaves the running
/// daemon on the old file until it re-executes.
pub(crate) const REPLACEABLE_WHILE_RUNNING: bool = true;

/// The binary this process runs, even once an installer replaced the path
/// it started from. Only Linux names it; elsewhere the daemon takes the
/// path's file at startup.
pub(in crate::daemon) fn running_identity() -> Option<FileIdentity> {
    #[cfg(target_os = "linux")]
    return fs::metadata("/proc/self/exe")
        .ok()
        .map(|metadata| FileIdentity::of(&metadata));
    #[cfg(not(target_os = "linux"))]
    None
}
