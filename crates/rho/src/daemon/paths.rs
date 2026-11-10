//! Where the daemon keeps its socket, lock, info file, and log.
//!
//! Each rho home has one daemon, and every path below belongs to exactly one
//! home: the lock and info file live in the home itself, and the socket lives
//! in a runtime directory named after the home. So two homes never share a
//! daemon file, and the lock alone decides which daemon owns a home.

use std::{
    ffi::OsString,
    fs, io,
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

/// Bytes in `sockaddr_un.sun_path`, which holds a Unix socket path plus its
/// NUL terminator.
#[cfg(any(target_os = "linux", target_os = "android"))]
const SUN_PATH_BYTES: usize = 108;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const SUN_PATH_BYTES: usize = 104;

/// Longest socket path `bind(2)` accepts on this platform.
const MAX_SOCKET_PATH_BYTES: usize = SUN_PATH_BYTES - 1;

/// Every file the daemon and its clients use.
///
/// [`DaemonPaths::resolve`] is pure: it takes the home's canonical path from
/// [`real_home`] and lays the files out under it. Both directories must be
/// private:
/// [`DaemonPaths::prepare_runtime_dir`] and [`DaemonPaths::prepare_state_dir`]
/// create or verify them before anything is created inside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DaemonPaths {
    /// The rho home's canonical path, which the rest lay out under.
    pub(crate) home: PathBuf,
    /// Holds the socket: `$XDG_RUNTIME_DIR/rho/<home id>` when
    /// `XDG_RUNTIME_DIR` is an absolute path, else `<rho home>/run`.
    pub(crate) runtime_dir: PathBuf,
    pub(crate) socket: PathBuf,
    /// `<rho home>/daemon`: the lock, the info file, and the daemon's own
    /// state, such as job records.
    pub(crate) state_dir: PathBuf,
    /// `<state dir>/daemon.lock`, held exclusively by the running daemon for
    /// its whole lifetime. It lives in the home, so daemons that resolve
    /// different runtime directories for one home (say, with different
    /// `XDG_RUNTIME_DIR`s) still exclude each other.
    pub(crate) lock: PathBuf,
    /// `<state dir>/daemon.json`: the JSON [`super::lock::DaemonRecord`]
    /// written after the socket is bound. It names the socket, so a daemon
    /// refused the lock can say where the running one listens.
    pub(crate) info_file: PathBuf,
    pub(crate) log_file: PathBuf,
}

/// Why the daemon's files cannot be used.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DaemonPathError {
    #[error(
        "rho daemon socket path {path} is {len} bytes, over the {max}-byte Unix socket path limit on this platform; point RHO_HOME or XDG_RUNTIME_DIR at a shorter directory"
    )]
    SocketPathTooLong {
        path: PathBuf,
        len: usize,
        max: usize,
    },
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

/// What [`DaemonPaths::inspect_socket`] found where the daemon listens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SocketFound {
    /// No daemon has bound a socket there, or its directory does not exist.
    Missing,
    /// A socket of this user's that nobody else can reach.
    Private,
}

impl DaemonPaths {
    /// Lays out the daemon files for `rho_home`, the home's canonical path
    /// from [`real_home`]. `env` looks up
    /// `XDG_RUNTIME_DIR`; tests inject it instead of reading process env.
    ///
    /// The per-user `$XDG_RUNTIME_DIR` is shared by every rho home of that
    /// user, so the socket goes in a subdirectory named after the home (see
    /// [`home_id`]); two homes never reach the same daemon.
    pub(crate) fn resolve(
        rho_home: &Path,
        env: impl Fn(&str) -> Option<OsString>,
    ) -> Result<Self, DaemonPathError> {
        // XDG: a relative value is invalid and must be ignored.
        let xdg_runtime_dir = env("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute());
        let runtime_dir = match xdg_runtime_dir {
            Some(dir) => dir.join("rho").join(home_id(rho_home)),
            None => rho_home.join("run"),
        };
        let socket = runtime_dir.join("daemon.sock");
        let len = socket.as_os_str().len();
        if len > MAX_SOCKET_PATH_BYTES {
            return Err(DaemonPathError::SocketPathTooLong {
                path: socket,
                len,
                max: MAX_SOCKET_PATH_BYTES,
            });
        }
        let state_dir = rho_home.join("daemon");
        Ok(Self {
            lock: state_dir.join("daemon.lock"),
            info_file: state_dir.join("daemon.json"),
            log_file: rho_home.join("logs").join("daemon.log"),
            state_dir,
            socket,
            runtime_dir,
            home: rho_home.to_owned(),
        })
    }

    /// Creates or verifies the runtime directory (see [`prepare_private_dir`]).
    /// Clients and the daemon both call this before touching the socket.
    pub(crate) fn prepare_runtime_dir(&self) -> Result<(), DaemonPathError> {
        prepare_private_dir(&self.runtime_dir)
    }

    /// Creates or verifies the state directory (see [`prepare_private_dir`]).
    /// Taking the daemon lock calls this first.
    pub(crate) fn prepare_state_dir(&self) -> Result<(), DaemonPathError> {
        prepare_private_dir(&self.state_dir)
    }

    /// Verifies, creating and changing nothing, that the socket and the
    /// runtime directory holding it are this user's and closed to everyone
    /// else, as [`DaemonPaths::prepare_runtime_dir`] and the daemon's bind
    /// leave them. `rho doctor` reports the result.
    pub(crate) fn inspect_socket(&self) -> Result<SocketFound, DaemonPathError> {
        let io_error = |path: &Path, error: io::Error| DaemonPathError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        };
        match fs::symlink_metadata(&self.runtime_dir) {
            Ok(metadata) => verify_private_dir(&self.runtime_dir, &metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SocketFound::Missing)
            }
            Err(error) => return Err(io_error(&self.runtime_dir, error)),
        }
        let path = &self.socket;
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SocketFound::Missing)
            }
            Err(error) => return Err(io_error(path, error)),
        };
        if !metadata.file_type().is_socket() {
            return Err(DaemonPathError::NotASocket { path: path.clone() });
        }
        let uid = current_uid();
        if metadata.uid() != uid {
            return Err(DaemonPathError::SocketWrongOwner {
                path: path.clone(),
                owner: metadata.uid(),
                uid,
            });
        }
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(DaemonPathError::InsecureSocketMode {
                path: path.clone(),
                mode,
            });
        }
        Ok(SocketFound::Private)
    }

    /// Creates the log directory (0700) and opens the log for appending
    /// (0600).
    pub(crate) fn open_log_file(&self) -> io::Result<fs::File> {
        if let Some(dir) = self.log_file.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.log_file)
    }
}

/// Creates `path` with mode 0700, or verifies an existing one is a real
/// directory owned by this user and closed to everyone else.
fn prepare_private_dir(path: &Path) -> Result<(), DaemonPathError> {
    let io_error = |error: io::Error| DaemonPathError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    };
    match fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
    {
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
fn verify_private_dir(path: &Path, metadata: &fs::Metadata) -> Result<(), DaemonPathError> {
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

/// Names a rho home's runtime subdirectory: the first 16 hex digits of the
/// SHA-256 of `rho_home`, the home's canonical path from [`real_home`]. Short,
/// so the socket path stays far under the Unix socket limit, and stable across
/// rho versions, so clients and daemons of different versions still find each
/// other.
fn home_id(rho_home: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    hex::encode(&Sha256::digest(rho_home.as_os_str().as_bytes())[..8])
}

/// Creates `rho_home` (mode 0700) if it is missing, then returns its canonical
/// path, which is what [`DaemonPaths::resolve`] takes.
///
/// Every spelling of one home (relative, a trailing slash, `//`, `.`, `..`, a
/// symlink) shares the home's one lock, so it must also name the same socket.
/// Canonicalizing only after the home exists keeps that true for a daemon
/// that creates its home, and keeps two relative homes in different
/// directories apart.
pub(crate) fn real_home(rho_home: &Path) -> io::Result<PathBuf> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(rho_home)?;
    fs::canonicalize(rho_home)
}

/// The effective uid of this process.
pub(crate) fn current_uid() -> u32 {
    // SAFETY: `geteuid` takes no pointers, has no preconditions, and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
