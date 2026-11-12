//! The daemon's endpoint: a Unix socket file, private by its mode and by
//! the 0700 runtime directory that holds it.

use std::{
    ffi::OsString,
    fs, io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use tokio::net::{UnixListener, UnixStream};

use super::files::{current_uid, verify_private_dir, DaemonPathError};
use crate::daemon::{
    paths::{home_id, DaemonPaths, SocketFound},
    platform::Endpoint,
};

/// Bytes in `sockaddr_un.sun_path`, which holds a Unix socket path plus its
/// NUL terminator.
#[cfg(any(target_os = "linux", target_os = "android"))]
const SUN_PATH_BYTES: usize = 108;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const SUN_PATH_BYTES: usize = 104;

/// Longest socket path `bind(2)` accepts on this platform.
pub(super) const MAX_SOCKET_PATH_BYTES: usize = SUN_PATH_BYTES - 1;

/// A connection between the daemon and one client.
pub(in crate::daemon) type Stream = UnixStream;

/// Where the daemon of `rho_home` listens: `daemon.sock` in
/// `$XDG_RUNTIME_DIR/rho/<home id>` when `env` gives `XDG_RUNTIME_DIR` as
/// an absolute path, else in `<rho home>/run`.
///
/// The per-user `$XDG_RUNTIME_DIR` is shared by every rho home of that
/// user, so the socket goes in a subdirectory named after the home (see
/// [`home_id`]); two homes never reach the same daemon.
pub(in crate::daemon) fn endpoint(
    rho_home: &Path,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<Endpoint, DaemonPathError> {
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
    Ok(Endpoint {
        runtime_dir,
        socket,
    })
}

/// The daemon's listening socket.
pub(in crate::daemon) struct Listener {
    pub(super) socket: UnixListener,
    uid: u32,
}

impl Listener {
    /// Listens on the socket of `paths`, after removing the one a crashed
    /// daemon left behind: the caller holds the daemon lock, so no daemon
    /// serves it. The runtime directory must already be prepared.
    pub(in crate::daemon) fn bind(paths: &DaemonPaths) -> io::Result<Self> {
        match fs::remove_file(&paths.socket) {
            Ok(()) => {
                tracing::info!(path = %paths.socket.display(), "removed stale daemon file");
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let socket = UnixListener::bind(&paths.socket)?;
        // The runtime directory is 0700, so nobody else can reach the socket in
        // the moment before this chmod; 0600 keeps it private if the directory's
        // mode is ever loosened.
        fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))?;
        Ok(Self::serving(socket))
    }

    pub(super) fn serving(socket: UnixListener) -> Self {
        Self {
            socket,
            uid: current_uid(),
        }
    }

    /// The next connection from the user who owns the daemon. Cancel safe:
    /// a connection is either returned or still waiting in the backlog.
    pub(in crate::daemon) async fn accept(&mut self) -> io::Result<Stream> {
        loop {
            let (stream, _) = self.socket.accept().await?;
            if peer_is_owner(&stream, self.uid) {
                return Ok(stream);
            }
        }
    }
}

/// Only the user who owns the daemon may connect. The socket's 0600 mode
/// already enforces this; the peer check keeps it true if the mode is ever
/// loosened.
fn peer_is_owner(stream: &UnixStream, uid: u32) -> bool {
    match stream.peer_cred() {
        Ok(credentials) if credentials.uid() == uid => true,
        Ok(credentials) => {
            tracing::warn!(
                peer_uid = credentials.uid(),
                "rejected connection from another user"
            );
            false
        }
        Err(error) => {
            tracing::warn!(%error, "rejected connection without peer credentials");
            false
        }
    }
}

/// Removes the socket file, which no daemon listens on any more.
pub(in crate::daemon) fn remove_endpoint(paths: &DaemonPaths) -> io::Result<()> {
    fs::remove_file(&paths.socket)
}

/// Connects to the daemon's socket. `None` when nothing listens there: no
/// socket, or one left by a daemon that died without cleanup.
pub(in crate::daemon) async fn connect(paths: &DaemonPaths) -> io::Result<Option<Stream>> {
    match UnixStream::connect(&paths.socket).await {
        Ok(stream) => Ok(Some(stream)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Connects to a daemon that is restarting. It keeps its socket through the
/// restart, so a connection made before the new binary accepts waits in the
/// socket's backlog, and nothing listening means the restart failed.
pub(in crate::daemon) async fn connect_restarting(
    paths: &DaemonPaths,
) -> io::Result<Option<Stream>> {
    connect(paths).await
}

/// Verifies, creating and changing nothing, that the socket and the
/// runtime directory holding it are this user's and closed to everyone
/// else, as preparing the runtime directory and [`Listener::bind`] leave
/// them.
pub(in crate::daemon) fn inspect_endpoint(
    paths: &DaemonPaths,
) -> Result<SocketFound, DaemonPathError> {
    let io_error = |path: &Path, error: io::Error| DaemonPathError::Unreadable {
        path: path.to_path_buf(),
        message: error.to_string(),
    };
    match fs::symlink_metadata(&paths.runtime_dir) {
        Ok(metadata) => verify_private_dir(&paths.runtime_dir, &metadata)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(SocketFound::Missing),
        Err(error) => return Err(io_error(&paths.runtime_dir, error)),
    }
    let path = &paths.socket;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(SocketFound::Missing),
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
