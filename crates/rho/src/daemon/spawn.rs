//! Connecting to the daemon, starting one first when none is running.

use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use rho_protocol::{ClientInfo, HelloResponse};

use super::{
    client::{connect, ConnectError, DaemonClient},
    paths::{DaemonPathError, DaemonPaths},
};

/// How long a client waits for a daemon it started to accept connections.
//
// Tripwire, not a tuning knob. Measured with the debug binary from exec to
// the first accepted connection, 15 runs each: median 7 ms idle, max 14 ms
// with all 24 cores saturated. 5 s leaves room for a cold page cache and slow
// CI hosts; hitting it means startup itself is broken.
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between connection attempts while a new daemon starts. Each
/// attempt is one `connect(2)` on a local socket.
const START_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Starts a daemon process for [`connect_or_spawn`].
///
/// Implementors launch the daemon and return without waiting for it:
/// `connect_or_spawn` detects readiness by connecting. The caller holds the
/// spawn lock, so at most one `spawn` runs per runtime directory at a time.
pub(crate) trait DaemonSpawner: Send + Sync {
    fn spawn(&self, paths: &DaemonPaths) -> io::Result<()>;
}

/// Starts `rho daemon run` from the current executable, detached from the
/// caller's terminal.
pub(crate) struct ProcessSpawner {
    executable: PathBuf,
    /// `RHO_HOME` for the daemon; `None` keeps the default data root. Read
    /// from the environment at the CLI boundary, not here.
    rho_home: Option<PathBuf>,
}

impl ProcessSpawner {
    pub(crate) fn current_exe(rho_home: Option<PathBuf>) -> io::Result<Self> {
        Ok(Self {
            executable: std::env::current_exe()?,
            rho_home,
        })
    }
}

impl DaemonSpawner for ProcessSpawner {
    fn spawn(&self, _paths: &DaemonPaths) -> io::Result<()> {
        let mut command = Command::new(&self.executable);
        command
            .args(["daemon", "run"])
            // Its own process group, so Ctrl+C in the caller's terminal does
            // not reach the daemon.
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(rho_home) = &self.rho_home {
            command.env("RHO_HOME", rho_home);
        }
        let mut child = command.spawn()?;
        // Reap the daemon if it exits while this process is still running.
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SpawnError {
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error(transparent)]
    Paths(#[from] DaemonPathError),
    #[error("could not take the rho daemon spawn lock {path}: {source}")]
    SpawnLock { path: PathBuf, source: io::Error },
    #[error("could not start the rho daemon: {0}")]
    Spawn(io::Error),
    #[error(
        "rho daemon did not accept connections within DAEMON_START_TIMEOUT ({limit:?}; waited {elapsed:?}); see {log_file}"
    )]
    StartTimeout {
        limit: Duration,
        elapsed: Duration,
        log_file: PathBuf,
    },
}

/// Connects to the daemon, starting one with `spawner` if none is running.
///
/// Concurrent callers serialize on the spawn lock: the first starts the
/// daemon, and the rest connect to it once they get the lock.
pub(crate) async fn connect_or_spawn(
    paths: &DaemonPaths,
    client: ClientInfo,
    spawner: &dyn DaemonSpawner,
) -> Result<(DaemonClient, HelloResponse), SpawnError> {
    match connect(paths, client.clone()).await {
        Err(ConnectError::NotRunning) => {}
        connected => return Ok(connected?),
    }
    paths.prepare_runtime_dir()?;
    // Serializes clients that race to start a daemon.
    let _spawn_lock = lock_spawn(&paths.runtime_dir.join("spawn.lock")).await?;
    // Another caller may have started the daemon while this one waited.
    match connect(paths, client.clone()).await {
        Err(ConnectError::NotRunning) => {}
        connected => return Ok(connected?),
    }
    spawner.spawn(paths).map_err(SpawnError::Spawn)?;
    let started = Instant::now();
    loop {
        match connect(paths, client.clone()).await {
            Err(ConnectError::NotRunning) => {}
            connected => return Ok(connected?),
        }
        let elapsed = started.elapsed();
        if elapsed >= DAEMON_START_TIMEOUT {
            return Err(SpawnError::StartTimeout {
                limit: DAEMON_START_TIMEOUT,
                elapsed,
                log_file: paths.log_file.clone(),
            });
        }
        tokio::time::sleep(START_POLL_INTERVAL).await;
    }
}

/// Blocks (off the async runtime) until this caller holds the spawn lock.
async fn lock_spawn(path: &Path) -> Result<File, SpawnError> {
    let lock_path = path.to_path_buf();
    let locked = tokio::task::spawn_blocking(move || -> io::Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(file)
    })
    .await
    .unwrap_or_else(|error| Err(io::Error::other(error)));
    locked.map_err(|source| SpawnError::SpawnLock {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
#[path = "spawn_tests.rs"]
mod tests;
