//! Connecting to the daemon, starting one first when none is running.

use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use rho_protocol::ClientInfo;

use super::{
    client::{connect, ConnectError, Connection},
    lock::{self, LockError},
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

/// How long a client waits for a daemon that holds the lock without
/// answering to answer or let go, before it starts its own.
//
// Tripwire. Such a daemon is stopping: it removed its socket, and keeps the
// lock until its turns save, at most `DRAIN_TIMEOUT` (4 s, in
// `daemon_host::sessions`) after the stop request, then exits in tens of
// milliseconds. 5 s matches `DAEMON_STOP_TIMEOUT`, which bounds the same
// wait for `rho daemon stop`. A daemon restarting in place (#1713) keeps its
// socket, so a client waits in its backlog instead and never comes here.
const DRAIN_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

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
///
/// The daemon inherits the caller's environment on purpose: `RHO_HOME` and
/// `XDG_RUNTIME_DIR` are what produced the caller's [`DaemonPaths`], so the
/// child resolves the same socket the caller then polls. Passing a different
/// home here would start a daemon the caller can never reach. It runs in `/`,
/// so `RHO_HOME` is pinned to the home's canonical path: a relative one would
/// otherwise name another directory there.
pub(crate) struct ProcessSpawner {
    executable: PathBuf,
}

impl ProcessSpawner {
    pub(crate) fn current_exe() -> io::Result<Self> {
        Ok(Self {
            executable: std::env::current_exe()?,
        })
    }
}

impl DaemonSpawner for ProcessSpawner {
    fn spawn(&self, paths: &DaemonPaths) -> io::Result<()> {
        let mut command = Command::new(&self.executable);
        command
            .args(["daemon", "run"])
            // Its own process group, so Ctrl+C in the caller's terminal does
            // not reach the daemon.
            .process_group(0)
            // Not the caller's directory, which the daemon would otherwise
            // hold for its whole life (blocking an unmount, say). It opens
            // each session in that session's own workspace.
            .current_dir("/")
            .env("RHO_HOME", &paths.home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // The caller's journal stream is not the daemon's stderr; the
            // daemon logs to its file instead.
            .env_remove("JOURNAL_STREAM")
            // A daemon a hook auto-started must not disable hooks for every
            // session; each session's own client decides (#1659).
            .env_remove(crate::hooks::IN_HOOK_ENV);
        // Nor run at the depth, or under the scope, of the session whose
        // `rho mcp serve` started it: each session's client sends its own.
        for name in crate::caller_context::ENV_NAMES {
            command.env_remove(name);
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
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error("could not start the rho daemon: {0}")]
    Spawn(io::Error),
    #[error(
        "rho daemon is still stopping after DRAIN_WAIT_TIMEOUT ({limit:?}; waited {elapsed:?}); see {log_file}"
    )]
    StillStopping {
        limit: Duration,
        elapsed: Duration,
        log_file: PathBuf,
    },
    #[error(
        "rho daemon (pid {pid}) listens on {}, not {}: this process has another XDG_RUNTIME_DIR than the one the daemon started with; set it to match",
        .socket.display(),
        .expected.display()
    )]
    OtherSocket {
        pid: u32,
        socket: PathBuf,
        expected: PathBuf,
    },
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
///
/// A daemon that is draining for a stop has already removed its socket but
/// still holds the daemon lock. A daemon spawned then
/// would exit as already running, so this waits for the old one to let go of
/// the lock first, up to [`DRAIN_WAIT_TIMEOUT`].
pub(crate) async fn connect_or_spawn(
    paths: &DaemonPaths,
    client: ClientInfo,
    spawner: &dyn DaemonSpawner,
) -> Result<Connection, SpawnError> {
    connect_or_spawn_observing(paths, client, spawner, || {}).await
}

/// The ensure path with an observer at the held-lock boundary, so lifecycle
/// tests can release a draining daemon only after a client encounters it.
async fn connect_or_spawn_observing(
    paths: &DaemonPaths,
    client: ClientInfo,
    spawner: &dyn DaemonSpawner,
    on_held: impl FnMut(),
) -> Result<Connection, SpawnError> {
    match connect(paths, client.clone()).await {
        Err(ConnectError::NotRunning) => {}
        connected => return Ok(connected?),
    }
    paths.prepare_runtime_dir()?;
    // Serializes clients that race to start a daemon.
    let _spawn_lock = lock_spawn(&paths.runtime_dir.join("spawn.lock")).await?;
    // Another caller may have started the daemon while this one waited.
    if let Some(connection) = wait_while_held(paths, &client, on_held).await? {
        return Ok(connection);
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

/// Connects to the daemon, or, while one holds the lock without answering,
/// waits until it answers or lets go. `None` once no daemon holds the lock:
/// the caller may start one.
///
/// A daemon holds the lock without answering while it starts, if a person
/// ran `rho daemon run`, and while it drains before it exits.
async fn wait_while_held(
    paths: &DaemonPaths,
    client: &ClientInfo,
    mut on_held: impl FnMut(),
) -> Result<Option<Connection>, SpawnError> {
    let started = Instant::now();
    loop {
        match connect(paths, client.clone()).await {
            Err(ConnectError::NotRunning) => {}
            connected => return Ok(Some(connected?)),
        }
        if !lock::is_held(paths)? {
            return Ok(None);
        }
        // A daemon that listens elsewhere never answers here: this process
        // resolved another runtime directory than the daemon did.
        if let Some(record) =
            lock::read_record(paths).filter(|record| record.socket != paths.socket)
        {
            return Err(SpawnError::OtherSocket {
                pid: record.pid,
                socket: record.socket,
                expected: paths.socket.clone(),
            });
        }
        on_held();
        let elapsed = started.elapsed();
        if elapsed >= DRAIN_WAIT_TIMEOUT {
            return Err(SpawnError::StillStopping {
                limit: DRAIN_WAIT_TIMEOUT,
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
