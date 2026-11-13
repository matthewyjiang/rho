//! Connecting to the daemon, starting one first when none is running, or
//! when the one running is older than this client and nobody is using it
//! (#1776).

use std::{
    fs::File,
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use rho_protocol::{ClientError, ClientHello};

use super::{
    client::{connect, ConnectError, Connection},
    lock::{self, LockError},
    paths::DaemonPaths,
    platform::{self, DaemonPathError},
    spawn_env::daemon_environment,
};

/// How long a client waits for a daemon it started to accept connections.
//
// Tripwire, not a tuning knob. Measured with the debug binary from exec to
// the first accepted connection, 15 runs each: median 7 ms idle, max 14 ms
// with all 24 cores saturated. 5 s leaves room for a cold page cache and slow
// CI hosts; hitting it means startup itself is broken.
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between connection attempts while a new daemon starts. Each
/// attempt is one connect on a local endpoint.
const START_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long a client waits for a daemon that holds the lock without
/// answering to answer or let go, before it starts its own.
//
// Tripwire. Such a daemon is stopping: it removed its socket, and keeps the
// lock until its turns save, at most `DRAIN_TIMEOUT` (4 s, in
// `daemon_host::sessions`) after the stop request, then exits in tens of
// milliseconds. 5 s matches `DAEMON_STOP_TIMEOUT`, which bounds the same
// wait for `rho daemon stop`. A daemon restarting in place (#1713) keeps its
// socket, so a client waits in its backlog instead and never comes here. An
// older daemon this client stops (#1776) is the same stop, bounded the same.
const DRAIN_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Starts a daemon process for [`connect_or_spawn`].
///
/// Implementors launch the daemon and resolve without waiting for it to
/// serve: `connect_or_spawn` detects readiness by connecting. The future
/// may take a bounded time to place the process it started, as
/// [`ProcessSpawner`] does to move it out of the caller's systemd unit; it
/// must not block the async runtime meanwhile. The caller holds the spawn
/// lock, so at most one `spawn` runs per runtime directory at a time.
pub(crate) trait DaemonSpawner: Send + Sync {
    /// Whether a daemon `spawn` starts could start, as far as the caller's
    /// side can tell. `connect_or_spawn` asks before every `spawn`, and
    /// before it stops an older daemon to start this one in its place, so a
    /// daemon that works is never stopped for one that cannot start.
    fn check(&self) -> io::Result<()> {
        Ok(())
    }

    fn spawn<'a>(&'a self, paths: &'a DaemonPaths) -> SpawnFuture<'a>;
}

/// What [`DaemonSpawner::spawn`] returns.
pub(crate) type SpawnFuture<'a> = Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>>;

/// Starts `rho daemon run` from the current executable, detached from the
/// caller's terminal and, where the platform can, from the caller's
/// service-manager unit.
///
/// The daemon does not inherit the caller's environment: it gets only the
/// variables that describe the daemon and the machine (see
/// [`super::spawn_env`]), so the first client to need a daemon does not
/// decide project trust, hook recursion, or session depth for every later
/// one (#1779). Each session takes those from its own client. A variable
/// that is not allowed never reaches the daemon; the daemon's own log is no
/// journal stream of the caller's either.
///
/// `RHO_HOME` and `XDG_RUNTIME_DIR` are what produced the caller's
/// [`DaemonPaths`], so the child resolves the same socket the caller then
/// polls. Passing a different home here would start a daemon the caller can
/// never reach. It does not run in the caller's directory, so `RHO_HOME` is
/// pinned to the home's canonical path: a relative one would otherwise name
/// another directory there.
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
    /// Spawns off the async runtime: placing the daemon waits on the
    /// service manager.
    fn spawn<'a>(&'a self, paths: &'a DaemonPaths) -> SpawnFuture<'a> {
        let mut command = Command::new(&self.executable);
        command
            .args(["daemon", "run"])
            .env_clear()
            .envs(daemon_environment(std::env::vars_os()))
            .env("RHO_HOME", &paths.home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let paths = paths.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut child = platform::spawn_detached(&mut command, &paths)?;
                // Reap the daemon if it exits while this process is still
                // running.
                std::thread::spawn(move || child.wait());
                Ok(())
            })
            .await
            .unwrap_or_else(|error| Err(io::Error::other(error)))
        })
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
    #[error("could not stop the older rho daemon to start this rho in its place: {0:#}")]
    StopOlder(anyhow::Error),
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
/// A daemon that refuses this client as newer than itself is replaced when
/// it reports that restarting it is free: this stops it and starts one with
/// `spawner` in its place, as `rho daemon restart` would, since that
/// interrupts nothing and disconnects no one. One with work in progress or
/// another client connected keeps running, and the refusal, which names
/// what a restart would cost, is the error (#1776). A daemon newer than
/// this client is never replaced.
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
    hello: ClientHello,
    spawner: &dyn DaemonSpawner,
) -> Result<Connection, SpawnError> {
    connect_or_spawn_observing(paths, hello, spawner, || {}).await
}

/// The ensure path with an observer at the held-lock boundary, so lifecycle
/// tests can release a draining daemon only after a client encounters it.
async fn connect_or_spawn_observing(
    paths: &DaemonPaths,
    hello: ClientHello,
    spawner: &dyn DaemonSpawner,
    on_held: impl FnMut(),
) -> Result<Connection, SpawnError> {
    match connect(paths, hello.clone()).await {
        Err(ConnectError::NotRunning) => {}
        Err(refusal) if is_unused_older_daemon(&refusal) => {}
        connected => return Ok(connected?),
    }
    paths.prepare_runtime_dir()?;
    // Serializes clients that race to start, or replace, a daemon.
    let _spawn_lock = lock_spawn(&paths.runtime_dir.join("spawn.lock")).await?;
    // Another caller may have started the daemon, or replaced the older
    // one, while this one waited, and the older one may be in use by now:
    // only what it answers under the lock decides.
    let unused_older = match wait_while_held(paths, &hello, on_held).await {
        Ok(Some(connection)) => return Ok(connection),
        Ok(None) => None,
        Err(SpawnError::Connect(refusal)) if is_unused_older_daemon(&refusal) => Some(refusal),
        Err(error) => return Err(error),
    };
    spawner.check().map_err(SpawnError::Spawn)?;
    if let Some(refusal) = unused_older {
        tracing::info!(%refusal, "replacing the unused rho daemon with this rho");
        stop_older(paths).await?;
    }
    spawner.spawn(paths).await.map_err(SpawnError::Spawn)?;
    let started = Instant::now();
    loop {
        match connect(paths, hello.clone()).await {
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

/// Whether the daemon refused this client as newer than itself, and nobody
/// uses it ([`rho_protocol::VersionMismatch::is_unused_older_daemon`]).
fn is_unused_older_daemon(refusal: &ConnectError) -> bool {
    matches!(
        refusal,
        ConnectError::Handshake(ClientError::VersionMismatch(mismatch))
            if mismatch.is_unused_older_daemon()
    )
}

/// Stops the older daemon that refused this client and waits until it lets
/// go of the lock, so the caller can start this rho in its place. It speaks
/// another protocol, so it cannot take a stop request and is asked outside
/// the protocol instead, which stops it as gracefully: work that began since
/// it reported none is recorded interrupted, as `rho daemon stop` records
/// it, and a client that connected since is detached as from any stop.
async fn stop_older(paths: &DaemonPaths) -> Result<(), SpawnError> {
    let asked = Instant::now();
    lock::terminate_recorded_daemon(paths).map_err(SpawnError::StopOlder)?;
    match lock::wait_until_released(paths, DRAIN_WAIT_TIMEOUT).await? {
        Some(_) => Ok(()),
        None => Err(SpawnError::StillStopping {
            limit: DRAIN_WAIT_TIMEOUT,
            elapsed: asked.elapsed(),
            log_file: paths.log_file.clone(),
        }),
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
    hello: &ClientHello,
    mut on_held: impl FnMut(),
) -> Result<Option<Connection>, SpawnError> {
    let started = Instant::now();
    loop {
        match connect(paths, hello.clone()).await {
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
        let file = platform::open_lock_file(&lock_path)?;
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
