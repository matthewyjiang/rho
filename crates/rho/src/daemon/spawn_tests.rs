use std::{
    os::unix::process::ExitStatusExt,
    process::ExitStatus,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    ErrorCode, Message, MessageReader, MessageWriter, ProtocolVersion, Response, RpcError,
    PROTOCOL_VERSION,
};
use tempfile::TempDir;
use tokio::{
    net::{UnixListener, UnixStream},
    task::JoinHandle,
};

use super::*;
use crate::daemon::{
    client::cli_client_info,
    lock, server,
    shutdown::{Ending, Shutdown},
    test_support::StubHost,
};

/// Failure bound for every racing caller to connect.
const RACE_DEADLINE: Duration = Duration::from_secs(30);

/// Starts the daemon inside the test process and counts how often it is asked.
#[derive(Default)]
struct InProcessSpawner {
    spawns: AtomicUsize,
    shutdown: Shutdown,
    servers: Mutex<Vec<JoinHandle<io::Result<()>>>>,
}

impl DaemonSpawner for InProcessSpawner {
    fn spawn(&self, paths: &DaemonPaths) -> io::Result<()> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        let lock = lock::acquire(paths).map_err(io::Error::other)?;
        let bound = server::bind(paths, lock)?;
        let host = StubHost::new(&self.shutdown);
        let shutdown = self.shutdown.clone();
        let served = tokio::spawn(async move { bound.serve(host, shutdown).await.release() });
        self.servers.lock().unwrap().push(served);
        Ok(())
    }
}

// Covers: clients racing to auto-spawn start exactly one daemon and all
// connect to it, instead of one winning and the rest failing on the lock.
// Owner: daemon auto-spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_clients_spawn_one_daemon_and_all_connect() {
    let root = TempDir::new().unwrap();
    let paths = DaemonPaths::resolve(root.path(), |_| None).unwrap();
    let spawner = Arc::new(InProcessSpawner {
        spawns: AtomicUsize::new(0),
        shutdown: Shutdown::default(),
        servers: Mutex::new(Vec::new()),
    });

    let callers: Vec<_> = (0..8)
        .map(|_| {
            let paths = paths.clone();
            let spawner = Arc::clone(&spawner);
            tokio::spawn(async move {
                connect_or_spawn(&paths, cli_client_info(), spawner.as_ref())
                    .await
                    .map(|connection| connection.hello.daemon.pid)
            })
        })
        .collect();
    let pids = tokio::time::timeout(RACE_DEADLINE, futures_util::future::join_all(callers))
        .await
        .expect("every caller connects");

    for pid in pids {
        assert_eq!(pid.unwrap().unwrap(), std::process::id());
    }
    assert_eq!(spawner.spawns.load(Ordering::SeqCst), 1);
    spawner.shutdown.request(Ending::Stop, |_| {});
    let servers = std::mem::take(&mut *spawner.servers.lock().unwrap());
    for served in servers {
        served.await.unwrap().unwrap();
    }
}

// Covers: a client that finds the lock held by a daemon that no longer
// answers, as one draining its turns before it exits does, waits for the
// lock instead of starting a daemon that would exit as already running,
// then starts one (#1730).
// Owner: daemon auto-spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_waits_for_a_draining_daemon_to_let_go_of_the_lock() {
    let root = TempDir::new().unwrap();
    let paths = DaemonPaths::resolve(root.path(), |_| None).unwrap();
    let spawner = Arc::new(InProcessSpawner {
        spawns: AtomicUsize::new(0),
        shutdown: Shutdown::default(),
        servers: Mutex::new(Vec::new()),
    });
    // The draining daemon: it holds the lock and removed its socket.
    let draining = lock::acquire(&paths).unwrap();
    let (observed, held) = tokio::sync::oneshot::channel();

    let caller = {
        let paths = paths.clone();
        let spawner = Arc::clone(&spawner);
        tokio::spawn(async move {
            let mut observed = Some(observed);
            connect_or_spawn_observing(&paths, cli_client_info(), spawner.as_ref(), || {
                if let Some(observed) = observed.take() {
                    let _ = observed.send(());
                }
            })
            .await
            .map(|connection| connection.hello.daemon.pid)
        })
    };
    tokio::time::timeout(RACE_DEADLINE, held)
        .await
        .expect("the caller reaches the held-lock wait")
        .expect("the caller observes the draining daemon");
    let waited = (caller.is_finished(), spawner.spawns.load(Ordering::SeqCst));
    drop(draining);
    let pid = tokio::time::timeout(RACE_DEADLINE, caller)
        .await
        .expect("the caller connects once the lock is free")
        .unwrap()
        .unwrap();

    assert_eq!(
        (waited, pid, spawner.spawns.load(Ordering::SeqCst)),
        ((false, 0), std::process::id(), 1)
    );
    spawner.shutdown.request(Ending::Stop, |_| {});
    let servers = std::mem::take(&mut *spawner.servers.lock().unwrap());
    for served in servers {
        served.await.unwrap().unwrap();
    }
}

// Covers: a client that resolved another socket than the running daemon,
// as one started with another XDG_RUNTIME_DIR does, is told so at once
// instead of waiting out the drain budget for a daemon that will never
// answer it, and starts none.
// Owner: daemon auto-spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_on_another_socket_is_told_where_the_daemon_listens() {
    let root = TempDir::new().unwrap();
    let paths = DaemonPaths::resolve(root.path(), |_| None).unwrap();
    let spawner = InProcessSpawner {
        spawns: AtomicUsize::new(0),
        shutdown: Shutdown::default(),
        servers: Mutex::new(Vec::new()),
    };
    let _running = lock::acquire(&paths).unwrap();
    let elsewhere = root.path().join("other-runtime/daemon.sock");
    lock::write_record(
        &paths,
        &lock::DaemonRecord {
            pid: 4242,
            version: "0.0.0".into(),
            protocol: rho_protocol::PROTOCOL_VERSION,
            started_at: 0,
            socket: elsewhere.clone(),
        },
    )
    .unwrap();

    let error = tokio::time::timeout(
        DRAIN_WAIT_TIMEOUT / 2,
        connect_or_spawn(&paths, cli_client_info(), &spawner),
    )
    .await
    .expect("the client is refused before the drain budget")
    .err()
    .expect("the client does not connect");

    assert_eq!(
        (
            error.to_string(),
            spawner.spawns.load(Ordering::SeqCst)
        ),
        (
            format!(
                "rho daemon (pid 4242) listens on {}, not {}: this process has another XDG_RUNTIME_DIR than the one the daemon started with; set it to match",
                elsewhere.display(),
                paths.socket.display()
            ),
            0
        )
    );
}

/// A daemon of another protocol version, as far as a client can tell: it
/// holds the daemon lock, answers every hello with `refusal`, and stops on
/// SIGTERM by removing its socket and letting go of the lock. The pid it
/// records is a child process's, so a signal a client sends the daemon lands
/// there, not on the test process.
struct RefusingDaemon {
    /// Resolves once the daemon stopped, with how its process ended.
    stopped: JoinHandle<ExitStatus>,
}

impl RefusingDaemon {
    fn start(paths: &DaemonPaths, refusal: VersionMismatch) -> Self {
        let lock = lock::acquire(paths).unwrap();
        paths.prepare_runtime_dir().unwrap();
        let listener = UnixListener::bind(&paths.socket).unwrap();
        let mut process = tokio::process::Command::new("sleep")
            .arg("600")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        lock::write_record(
            paths,
            &lock::DaemonRecord {
                pid: process.id().expect("the process is running"),
                version: "0.0.0".into(),
                protocol: refusal.daemon,
                started_at: 0,
                socket: paths.socket.clone(),
            },
        )
        .unwrap();
        let socket = paths.socket.clone();
        let stopped = tokio::spawn(async move {
            let _lock = lock;
            loop {
                tokio::select! {
                    ended = process.wait() => {
                        std::fs::remove_file(&socket).unwrap();
                        return ended.unwrap();
                    }
                    accepted = listener.accept() => {
                        tokio::spawn(refuse(accepted.unwrap().0, refusal));
                    }
                }
            }
        });
        Self { stopped }
    }
}

/// Answers the hello on `stream` with `refusal`.
async fn refuse(stream: UnixStream, refusal: VersionMismatch) {
    let (read, write) = stream.into_split();
    let hello = MessageReader::new(read).read_line().await.unwrap();
    let Message::Request(hello) = Message::parse(&hello.expect("a hello")).unwrap() else {
        panic!("expected a request");
    };
    let refused = RpcError::new(ErrorCode::VersionMismatch, refusal.to_string())
        .with_data(serde_json::to_value(refusal).unwrap());
    // The client may leave as soon as it has read the refusal.
    let _ = MessageWriter::new(write)
        .write(&Message::Response(Response {
            id: Some(hello.id),
            outcome: Err(refused),
        }))
        .await;
}

/// The refusal of a daemon one protocol version older than this client,
/// with `in_progress` things a restart would interrupt.
fn older_daemon(in_progress: Option<u64>) -> VersionMismatch {
    VersionMismatch {
        daemon: ProtocolVersion::new(0, 0),
        client: PROTOCOL_VERSION,
        fix: VersionFix::RestartDaemon,
        in_progress,
    }
}

// Covers: a client that finds a daemon older than itself with nothing in
// progress stops it and starts its own in its place, then connects to that
// one, instead of failing on a daemon nobody is using (#1776).
// Owner: daemon auto-spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_older_daemon_is_replaced_by_the_clients_own() {
    let root = TempDir::new().unwrap();
    let paths = DaemonPaths::resolve(root.path(), |_| None).unwrap();
    let spawner = InProcessSpawner::default();
    let older = RefusingDaemon::start(&paths, older_daemon(Some(0)));

    let pid = tokio::time::timeout(
        RACE_DEADLINE,
        connect_or_spawn(&paths, cli_client_info(), &spawner),
    )
    .await
    .expect("the client connects once the older daemon stopped")
    .map(|connection| connection.hello.daemon.pid)
    .unwrap();
    let stopped_by = tokio::time::timeout(RACE_DEADLINE, older.stopped)
        .await
        .expect("the older daemon stops")
        .unwrap()
        .signal();

    assert_eq!(
        (pid, spawner.spawns.load(Ordering::SeqCst), stopped_by),
        (std::process::id(), 1, Some(libc::SIGTERM))
    );
    spawner.shutdown.request(Ending::Stop, |_| {});
    let servers = std::mem::take(&mut *spawner.servers.lock().unwrap());
    for served in servers {
        served.await.unwrap().unwrap();
    }
}

// Covers: a client replaces no daemon it cannot tell is older and idle: one
// with work in progress, one that does not say what it has in progress, and
// one newer than the client all keep running, the client starts no daemon,
// and its error is the daemon's refusal, which names the fix and what a
// restart would interrupt (#1776).
// Owner: daemon auto-spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_that_is_busy_or_newer_keeps_running_and_its_refusal_is_the_error() {
    let newer_daemon = VersionMismatch {
        daemon: ProtocolVersion::new(PROTOCOL_VERSION.major + 1, 0),
        client: PROTOCOL_VERSION,
        fix: VersionFix::UpdateClient,
        in_progress: None,
    };
    for refusal in [older_daemon(Some(2)), older_daemon(None), newer_daemon] {
        let root = TempDir::new().unwrap();
        let paths = DaemonPaths::resolve(root.path(), |_| None).unwrap();
        let spawner = InProcessSpawner::default();
        let _daemon = RefusingDaemon::start(&paths, refusal);

        let error = tokio::time::timeout(
            RACE_DEADLINE,
            connect_or_spawn(&paths, cli_client_info(), &spawner),
        )
        .await
        .expect("the client is refused")
        .err()
        .expect("the client does not connect");

        let SpawnError::Connect(ConnectError::Handshake(ClientError::VersionMismatch(reported))) =
            error
        else {
            panic!("expected the daemon's refusal, got {error:?}");
        };
        assert_eq!(
            (
                reported,
                lock::is_held(&paths).unwrap(),
                spawner.spawns.load(Ordering::SeqCst)
            ),
            (refusal, true, 0)
        );
    }
}
