//! A frontend's way to its rho home's daemon: connect to the one that runs,
//! or start it the way the `rho` CLI does.

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
};

use tokio::net::UnixStream;

use crate::{
    Client, ClientError, ClientHello, DaemonEndpoint, HelloResponse, Notifications,
    SocketPathTooLong,
};

/// The daemon of one rho home, as a frontend reaches it.
///
/// [`LocalDaemon::connect`] finds a daemon that runs and never starts one.
/// [`LocalDaemon::connect_or_start`] starts it first when none runs, by
/// running `rho daemon start`, which is the ensure-running path every `rho`
/// client takes: it serializes clients that race to start the daemon, waits
/// out one that is stopping, replaces an older one nobody uses, and starts
/// it with only the environment a daemon keeps (#1779). So a frontend
/// starts the same daemon the `rho` it names would.
///
/// The daemon belongs to the rho home and outlives the frontend that
/// started it; `rho daemon stop` stops it.
#[derive(Clone, Debug)]
pub struct LocalDaemon {
    rho_home: PathBuf,
    xdg_runtime_dir: Option<PathBuf>,
}

/// Why [`LocalDaemon`] reached no daemon.
#[derive(Debug, thiserror::Error)]
pub enum LocalDaemonError {
    /// The rho home exists but its path cannot be resolved.
    #[error("could not resolve rho home {path}: {source}")]
    Home { path: PathBuf, source: io::Error },
    #[error(transparent)]
    SocketPathTooLong(#[from] SocketPathTooLong),
    /// Something listens on the socket, but connecting to it failed.
    #[error("could not connect to the rho daemon at {socket}: {source}")]
    Connect { socket: PathBuf, source: io::Error },
    /// The daemon answered, but the handshake failed. A
    /// [`ClientError::VersionMismatch`] says which side to change.
    #[error(transparent)]
    Handshake(#[from] ClientError),
    #[error("could not run `{program} daemon start`: {source}", program = .program.display())]
    Run { program: PathBuf, source: io::Error },
    /// `rho daemon start` failed; `stderr` is what it said, which names the
    /// daemon's log when the daemon did not start.
    #[error("`{program} daemon start` failed ({status}): {stderr}", program = .program.display())]
    Start {
        program: PathBuf,
        status: ExitStatus,
        stderr: String,
    },
    /// `rho daemon start` reported a daemon, but none listens where this
    /// client looks, as when that `rho` lays its files out differently.
    #[error(
        "`{program} daemon start` succeeded, but no rho daemon listens for {rho_home}",
        program = .program.display(),
        rho_home = .rho_home.display()
    )]
    NotListening { program: PathBuf, rho_home: PathBuf },
}

impl LocalDaemon {
    /// The daemon of the rho home at `rho_home`, placed by this process's
    /// `XDG_RUNTIME_DIR` as the `rho` CLI places it.
    pub fn new(rho_home: impl Into<PathBuf>) -> Self {
        Self {
            rho_home: rho_home.into(),
            xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        }
    }

    /// Places the socket by `xdg_runtime_dir` instead of this process's
    /// `XDG_RUNTIME_DIR`; `None` acts as if it were unset. A daemon this
    /// starts gets the same value, so it listens where this looks.
    pub fn with_xdg_runtime_dir(mut self, xdg_runtime_dir: Option<PathBuf>) -> Self {
        self.xdg_runtime_dir = xdg_runtime_dir;
        self
    }

    /// Where the daemon listens, or `None` while the rho home does not
    /// exist, so no daemon can run there.
    pub fn endpoint(&self) -> Result<Option<DaemonEndpoint>, LocalDaemonError> {
        // The daemon names its socket after its home's canonical path.
        let rho_home = match std::fs::canonicalize(&self.rho_home) {
            Ok(rho_home) => rho_home,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(LocalDaemonError::Home {
                    path: self.rho_home.clone(),
                    source,
                })
            }
        };
        Ok(Some(DaemonEndpoint::locate(
            &rho_home,
            self.xdg_runtime_dir.as_deref(),
        )?))
    }

    /// Connects to the daemon that runs and completes the handshake as
    /// `hello`, or `None` when none runs. Never starts a daemon.
    pub async fn connect(
        &self,
        hello: ClientHello,
    ) -> Result<Option<(Client, Notifications, HelloResponse)>, LocalDaemonError> {
        let Some(DaemonEndpoint { socket, .. }) = self.endpoint()? else {
            return Ok(None);
        };
        let stream = match UnixStream::connect(&socket).await {
            Ok(stream) => stream,
            // No socket, or one a daemon that died without cleanup left.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None)
            }
            Err(source) => return Err(LocalDaemonError::Connect { socket, source }),
        };
        Ok(Some(Client::handshake(stream, hello).await?))
    }

    /// Connects to the daemon as [`Self::connect`] does, first starting it
    /// with `rho daemon start` when none runs or the one running is older
    /// than this client and nobody uses it. `rho` runs `rho`: the caller
    /// sets its program and environment, and this adds the arguments,
    /// `RHO_HOME`, `XDG_RUNTIME_DIR`, and its standard streams.
    pub async fn connect_or_start(
        &self,
        rho: std::process::Command,
        hello: ClientHello,
    ) -> Result<(Client, Notifications, HelloResponse), LocalDaemonError> {
        match self.connect(hello.clone()).await {
            Ok(Some(connected)) => return Ok(connected),
            Ok(None) => {}
            // `rho daemon start` replaces it, as every rho client does.
            Err(LocalDaemonError::Handshake(ClientError::VersionMismatch(refusal)))
                if refusal.is_unused_older_daemon() => {}
            Err(error) => return Err(error),
        }
        let program = PathBuf::from(rho.get_program());
        self.start(rho).await?;
        self.connect(hello)
            .await?
            .ok_or_else(|| LocalDaemonError::NotListening {
                program,
                rho_home: self.rho_home.clone(),
            })
    }

    /// Runs `rho daemon start` for this home, which returns once the daemon
    /// answers.
    async fn start(&self, rho: std::process::Command) -> Result<(), LocalDaemonError> {
        let program = PathBuf::from(rho.get_program());
        let mut command = tokio::process::Command::from(rho);
        command
            .args(["daemon", "start"])
            // Absolute, so a `rho` run in another directory finds this home.
            .env("RHO_HOME", absolute(&self.rho_home))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        match &self.xdg_runtime_dir {
            Some(dir) => command.env("XDG_RUNTIME_DIR", dir),
            None => command.env_remove("XDG_RUNTIME_DIR"),
        };
        let output = command
            .output()
            .await
            .map_err(|source| LocalDaemonError::Run {
                program: program.clone(),
                source,
            })?;
        if output.status.success() {
            return Ok(());
        }
        Err(LocalDaemonError::Start {
            program,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

/// `path` made absolute against this process's directory, or as it is when
/// that directory cannot be read.
fn absolute(path: &Path) -> OsString {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .into_os_string()
}

#[cfg(test)]
#[path = "local_daemon_tests.rs"]
mod tests;
