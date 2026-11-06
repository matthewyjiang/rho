//! Where the daemon keeps its socket, lock, info file, and log.

use std::{
    ffi::OsString,
    fs, io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

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
/// [`DaemonPaths::resolve`] is pure. The runtime directory holds the
/// rendezvous files (socket and info file) and must be private:
/// [`DaemonPaths::prepare_runtime_dir`] creates or verifies it before anything
/// is created inside. The lock lives in the rho home instead, because the
/// state it guards (the job records under `<rho home>/daemon/jobs`) belongs to
/// the home, not to whichever runtime directory a daemon happened to resolve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DaemonPaths {
    /// `<rho home>/run` for an explicit `RHO_HOME`, else
    /// `$XDG_RUNTIME_DIR/rho`, else `~/.rho/run`.
    pub(crate) runtime_dir: PathBuf,
    pub(crate) socket: PathBuf,
    /// `<rho home>/daemon/daemon.lock`, held exclusively by the running daemon
    /// for its whole lifetime. One per rho home, so two daemons that resolve
    /// different runtime directories for the same home (say, with different
    /// `XDG_RUNTIME_DIR`s) still exclude each other.
    pub(crate) lock: PathBuf,
    /// JSON [`super::lock::DaemonRecord`] written after the socket is bound.
    pub(crate) info_file: PathBuf,
    pub(crate) log_file: PathBuf,
}

/// Why the daemon's files cannot be used.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DaemonPathError {
    #[error(
        "rho daemon socket path {path} is {len} bytes, over the {max}-byte Unix socket path limit on this platform; point RHO_HOME (or XDG_RUNTIME_DIR for the default home) at a shorter directory"
    )]
    SocketPathTooLong {
        path: PathBuf,
        len: usize,
        max: usize,
    },
    #[error("rho daemon runtime directory {path} is not a directory; remove it and retry")]
    NotADirectory { path: PathBuf },
    #[error(
        "rho daemon runtime directory {path} is owned by uid {owner}, not uid {uid}; remove it and retry"
    )]
    WrongOwner { path: PathBuf, owner: u32, uid: u32 },
    #[error(
        "rho daemon runtime directory {path} has mode {mode:04o} but must not be accessible to group or others; run `chmod 700 {path}`"
    )]
    InsecureMode { path: PathBuf, mode: u32 },
    #[error("could not prepare rho daemon directory {path}: {message}")]
    Io { path: PathBuf, message: String },
}

impl DaemonPaths {
    /// Lays out the daemon files for `rho_home`. `env` looks up `RHO_HOME`
    /// and `XDG_RUNTIME_DIR`; tests inject it instead of reading process env.
    ///
    /// A daemon serves exactly one rho home, so an explicit `RHO_HOME` keeps
    /// its socket inside that home. Only the default home uses the shared
    /// per-user `$XDG_RUNTIME_DIR`; otherwise two homes would reach the same
    /// daemon and mix their sessions and config.
    pub(crate) fn resolve(
        rho_home: &Path,
        env: impl Fn(&str) -> Option<OsString>,
    ) -> Result<Self, DaemonPathError> {
        let explicit_home = env("RHO_HOME").is_some_and(|value| !value.is_empty());
        // XDG: a relative value is invalid and must be ignored.
        let xdg_runtime_dir = env("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute());
        let runtime_dir = match xdg_runtime_dir {
            Some(dir) if !explicit_home => dir.join("rho"),
            _ => rho_home.join("run"),
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
        Ok(Self {
            lock: rho_home.join("daemon").join("daemon.lock"),
            info_file: runtime_dir.join("daemon.json"),
            log_file: rho_home.join("logs").join("daemon.log"),
            socket,
            runtime_dir,
        })
    }

    /// Creates the runtime directory with mode 0700, or verifies an existing
    /// one is a real directory owned by this user and closed to everyone else.
    /// Clients and the daemon both call this before touching files inside.
    pub(crate) fn prepare_runtime_dir(&self) -> Result<(), DaemonPathError> {
        let path = &self.runtime_dir;
        let io_error = |error: io::Error| DaemonPathError::Io {
            path: path.clone(),
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
        if !metadata.is_dir() {
            return Err(DaemonPathError::NotADirectory { path: path.clone() });
        }
        let uid = current_uid();
        if metadata.uid() != uid {
            return Err(DaemonPathError::WrongOwner {
                path: path.clone(),
                owner: metadata.uid(),
                uid,
            });
        }
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(DaemonPathError::InsecureMode {
                path: path.clone(),
                mode,
            });
        }
        Ok(())
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

/// The effective uid of this process.
pub(crate) fn current_uid() -> u32 {
    // SAFETY: `geteuid` takes no pointers, has no preconditions, and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
