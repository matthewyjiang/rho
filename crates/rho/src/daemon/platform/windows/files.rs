//! The daemon's directories and files, and which file a path names.
//!
//! They sit in the rho home, which is private by the access control list of
//! the user's profile, as the credentials and sessions beside them are.

use std::{
    fs::{self, File, OpenOptions},
    io,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Why the daemon's files cannot be used.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DaemonPathError {
    /// Never on Windows, whose pipe names have no length limit; here
    /// because `rho_protocol::DaemonEndpoint::locate` returns it everywhere.
    #[error(transparent)]
    SocketPathTooLong(#[from] rho_protocol::SocketPathTooLong),
    #[error("rho daemon directory {path} is not a directory; remove it and retry")]
    NotADirectory { path: PathBuf },
    #[error("could not prepare rho daemon directory {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("could not read {path}: {message}")]
    Unreadable { path: PathBuf, message: String },
    #[error(
        "rho daemon pipe {path} was created by another user; find the process that holds it, end it, and let the next client start the daemon"
    )]
    ForeignPipe { path: PathBuf },
}

impl DaemonPathError {
    /// Whether the files could not be checked at all, rather than checked
    /// and found open to someone else or unusable.
    pub(crate) fn is_unreadable(&self) -> bool {
        match self {
            Self::Io { .. } | Self::Unreadable { .. } => true,
            Self::SocketPathTooLong(_) | Self::NotADirectory { .. } | Self::ForeignPipe { .. } => {
                false
            }
        }
    }
}

/// Creates `path` and its missing parents. An existing directory is left as
/// it is.
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

/// Creates `path`, or verifies an existing one is a real directory.
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
    // Not following links: a symlink or a junction could point anywhere.
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.is_dir() || metadata.is_symlink() {
        return Err(DaemonPathError::NotADirectory {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Opens the lock file `path` for reading and writing, creating it if it
/// is missing.
pub(in crate::daemon) fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Opens the log file `path` for appending, creating it if it is missing.
pub(in crate::daemon) fn open_log_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// Nothing to flush: Windows does not open a directory as a file, and NTFS
/// journals the directory entries a create, rename, or remove changes.
pub(crate) fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Opens `path` for writing, emptied, creating it if it is missing.
pub(crate) fn create_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

/// Which file a path names: equal identities are the same file, unchanged.
/// An installer replaces a running binary by renaming it away and writing a
/// new file, which has its own creation time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    created: u64,
    modified: u64,
    size: u64,
}

impl FileIdentity {
    pub(crate) fn of(metadata: &fs::Metadata) -> Self {
        Self {
            created: metadata.creation_time(),
            modified: metadata.last_write_time(),
            size: metadata.file_size(),
        }
    }
}

/// Whether an installer can put a new executable at the daemon's path while
/// the daemon runs. It cannot: Windows refuses to overwrite or delete the
/// file of a running program.
pub(crate) const REPLACEABLE_WHILE_RUNNING: bool = false;

/// Nothing names the binary this process runs apart from its path, so the
/// daemon takes the path's file at startup.
pub(in crate::daemon) fn running_identity() -> Option<FileIdentity> {
    None
}
