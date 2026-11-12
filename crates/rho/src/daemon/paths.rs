//! Where the daemon keeps its socket, lock, info file, and log.
//!
//! Each rho home has one daemon, and every path below belongs to exactly one
//! home: the lock and info file live in the home itself, and the socket lives
//! in a runtime directory named after the home. So two homes never share a
//! daemon file, and the lock alone decides which daemon owns a home.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::platform::{self, DaemonPathError, Endpoint};

/// The variables besides `HOME` that decide which daemon a client reaches:
/// `RHO_HOME` picks the home, and on Unix `XDG_RUNTIME_DIR` the directory
/// of its socket (see [`DaemonPaths::resolve`]). A process given a client's
/// values reaches that client's daemon.
pub(crate) const DAEMON_LOCATION_ENV: [&str; 2] = ["RHO_HOME", "XDG_RUNTIME_DIR"];

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
    /// Holds the spawn lock and, where the endpoint is a file, the endpoint.
    /// The platform decides where it is (see [`platform::endpoint`]).
    pub(crate) runtime_dir: PathBuf,
    /// What clients connect to.
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
    /// from [`real_home`]. `env` looks up the variables that place the
    /// endpoint; tests inject it instead of reading process env.
    pub(crate) fn resolve(
        rho_home: &Path,
        env: impl Fn(&str) -> Option<OsString>,
    ) -> Result<Self, DaemonPathError> {
        let Endpoint {
            runtime_dir,
            socket,
        } = platform::endpoint(rho_home, env)?;
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

    /// Creates the runtime directory, or verifies that an existing one is
    /// private to this user. Clients and the daemon both call this before
    /// touching the socket.
    pub(crate) fn prepare_runtime_dir(&self) -> Result<(), DaemonPathError> {
        platform::prepare_private_dir(&self.runtime_dir)
    }

    /// Creates the state directory, or verifies that an existing one is
    /// private to this user. Taking the daemon lock calls this first.
    pub(crate) fn prepare_state_dir(&self) -> Result<(), DaemonPathError> {
        platform::prepare_private_dir(&self.state_dir)
    }

    /// Verifies, creating and changing nothing, that the endpoint and the
    /// runtime directory are this user's and closed to everyone else, as
    /// [`DaemonPaths::prepare_runtime_dir`] and the daemon's bind leave
    /// them. `rho doctor` reports the result.
    pub(crate) fn inspect_socket(&self) -> Result<SocketFound, DaemonPathError> {
        platform::inspect_endpoint(self)
    }

    /// Creates the log directory and opens the log for appending, both
    /// private to this user.
    pub(crate) fn open_log_file(&self) -> io::Result<fs::File> {
        if let Some(dir) = self.log_file.parent() {
            platform::create_private_dir(dir)?;
        }
        platform::open_log_file(&self.log_file)
    }
}

/// Names a rho home's runtime subdirectory: the first 16 hex digits of the
/// SHA-256 of `rho_home`, the home's canonical path from [`real_home`]. Short,
/// so the endpoint's name stays far under the platform's limit, and stable across
/// rho versions, so clients and daemons of different versions still find each
/// other.
pub(super) fn home_id(rho_home: &Path) -> String {
    hex::encode(&Sha256::digest(platform::path_bytes(rho_home))[..8])
}

/// Creates `rho_home`, private to this user, if it is missing, then returns
/// its canonical path, which is what [`DaemonPaths::resolve`] takes.
///
/// Every spelling of one home (relative, a trailing slash, `//`, `.`, `..`, a
/// symlink) shares the home's one lock, so it must also name the same socket.
/// Canonicalizing only after the home exists keeps that true for a daemon
/// that creates its home, and keeps two relative homes in different
/// directories apart.
pub(crate) fn real_home(rho_home: &Path) -> io::Result<PathBuf> {
    platform::create_private_dir(rho_home)?;
    fs::canonicalize(rho_home)
}
