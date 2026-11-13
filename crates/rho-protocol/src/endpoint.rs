//! Where a rho home's daemon listens. The daemon binds here and every client
//! connects here, so this one layout is shared by the `rho` CLI, the daemon,
//! and frontends such as rho-ui (see [`crate::LocalDaemon`]).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Where the daemon of one rho home listens.
///
/// Every path belongs to exactly one home: the endpoint is named after the
/// home (see [`DaemonEndpoint::locate`]), so two homes never reach the same
/// daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DaemonEndpoint {
    /// The private directory that holds the daemon's spawn lock and, on
    /// Unix, its socket.
    pub runtime_dir: PathBuf,
    /// What clients connect to: a socket file on Unix, a named pipe on
    /// Windows.
    pub socket: PathBuf,
}

/// A Unix socket path over the platform's limit, which `bind(2)` would
/// truncate. Never returned on Windows, whose pipe names have no such limit.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
#[error(
    "rho daemon socket path {path} is {len} bytes, over the {max}-byte Unix socket path limit on this platform; point RHO_HOME or XDG_RUNTIME_DIR at a shorter directory"
)]
pub struct SocketPathTooLong {
    pub path: PathBuf,
    pub len: usize,
    pub max: usize,
}

/// Bytes in `sockaddr_un.sun_path`, which holds a Unix socket path plus its
/// NUL terminator.
#[cfg(any(target_os = "linux", target_os = "android"))]
const SUN_PATH_BYTES: usize = 108;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const SUN_PATH_BYTES: usize = 104;

/// Longest socket path `bind(2)` accepts on this platform.
#[cfg(unix)]
const MAX_SOCKET_PATH_BYTES: usize = SUN_PATH_BYTES - 1;

impl DaemonEndpoint {
    /// Where the daemon of `rho_home`, the home's canonical path, listens,
    /// so every spelling of one home finds one endpoint.
    ///
    /// On Unix: `daemon.sock` in `$XDG_RUNTIME_DIR/rho/<home id>` when
    /// `xdg_runtime_dir` is an absolute path, else in `<rho home>/run`. The
    /// per-user `$XDG_RUNTIME_DIR` is shared by every rho home of that user,
    /// so the socket goes in a subdirectory named after the home; two homes
    /// never reach the same daemon. A relative `XDG_RUNTIME_DIR` is invalid
    /// by the XDG spec and ignored.
    ///
    /// On Windows: the pipe `\\.\pipe\rho-daemon-<home id>`, and
    /// `xdg_runtime_dir` is ignored. Pipes live in one namespace for the
    /// whole machine and not in a directory, so the runtime directory,
    /// `<rho home>\run`, holds only the spawn lock.
    pub fn locate(
        rho_home: &Path,
        xdg_runtime_dir: Option<&Path>,
    ) -> Result<Self, SocketPathTooLong> {
        #[cfg(unix)]
        {
            let runtime_dir = match xdg_runtime_dir.filter(|dir| dir.is_absolute()) {
                Some(dir) => dir.join("rho").join(home_id(rho_home)),
                None => rho_home.join("run"),
            };
            let socket = runtime_dir.join("daemon.sock");
            let len = socket.as_os_str().len();
            if len > MAX_SOCKET_PATH_BYTES {
                return Err(SocketPathTooLong {
                    path: socket,
                    len,
                    max: MAX_SOCKET_PATH_BYTES,
                });
            }
            Ok(Self {
                runtime_dir,
                socket,
            })
        }
        #[cfg(windows)]
        {
            let _ = xdg_runtime_dir;
            Ok(Self {
                runtime_dir: rho_home.join("run"),
                socket: format!(r"\\.\pipe\rho-daemon-{}", home_id(rho_home)).into(),
            })
        }
    }
}

/// Names a rho home's endpoint: the first 16 hex digits of the SHA-256 of
/// `rho_home`, the home's canonical path. Short, so the endpoint's name stays
/// far under the platform's limit, and stable across rho versions, so
/// clients and daemons of different versions still find each other.
fn home_id(rho_home: &Path) -> String {
    Sha256::digest(path_bytes(rho_home))[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The bytes that name `path`, the same for one path in every rho version,
/// so what is derived from them stays stable.
#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

/// The bytes that name `path`, the same for one path in every rho version,
/// so what is derived from them stays stable: its UTF-16 code units, little
/// endian.
#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(all(test, unix))]
#[path = "endpoint_tests.rs"]
mod tests;
