use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_protocol::{ProtocolVersion, RestartCost, VersionFix, VersionMismatch, PROTOCOL_VERSION};
use tempfile::TempDir;
use tokio::task::JoinHandle;

use super::*;
use crate::daemon::{
    client::ConnectAs,
    lock, server,
    shutdown::{Ending, Shutdown},
    test_support::{RefusingListener, StopTarget, StubHost},
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
    fn spawn<'a>(&'a self, paths: &'a DaemonPaths) -> SpawnFuture<'a> {
        Box::pin(std::future::ready(self.serve(paths)))
    }
}

impl InProcessSpawner {
    fn serve(&self, paths: &DaemonPaths) -> io::Result<()> {
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
                connect_or_spawn(&paths, ConnectAs::Command.hello(), spawner.as_ref())
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
            connect_or_spawn_observing(&paths, ConnectAs::Command.hello(), spawner.as_ref(), || {
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
        connect_or_spawn(&paths, ConnectAs::Command.hello(), &spawner),
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
/// holds the daemon lock, answers each hello with the next of `refusals`,
/// repeating the last, and stops when asked outside the protocol, by
/// removing its socket and letting go of the lock.
struct RefusingDaemon {
    /// Resolves once the daemon was asked to stop, and stopped.
    stopped: JoinHandle<()>,
}

impl RefusingDaemon {
    fn start(paths: &DaemonPaths, refusals: &[VersionMismatch]) -> Self {
        let lock = lock::acquire(paths).unwrap();
        let mut listener = RefusingListener::bind(paths);
        let mut stop = StopTarget::listen(paths);
        lock::write_record(
            paths,
            &lock::DaemonRecord {
                pid: stop.pid(),
                version: "0.0.0".into(),
                protocol: refusals[0].daemon,
                started_at: 0,
                socket: paths.socket.clone(),
            },
        )
        .unwrap();
        let paths = paths.clone();
        let refusals = refusals.to_vec();
        let mut answered = 0;
        let stopped = tokio::spawn(async move {
            let _lock = lock;
            loop {
                let refusal = refusals[answered.min(refusals.len() - 1)];
                tokio::select! {
                    () = stop.asked() => {
                        drop(listener);
                        platform::remove_endpoint(&paths).unwrap();
                        return;
                    }
                    // In turn, so the hellos are answered in order.
                    () = listener.refuse_next(refusal) => answered += 1,
                }
            }
        });
        Self { stopped }
    }
}

/// The refusal of a daemon one protocol version older than this client,
/// whose restart costs `restart`.
fn older_daemon(restart: Option<RestartCost>) -> VersionMismatch {
    VersionMismatch {
        daemon: ProtocolVersion::new(0, 0),
        client: PROTOCOL_VERSION,
        fix: VersionFix::RestartDaemon,
        restart,
    }
}

const UNUSED: RestartCost = RestartCost {
    in_progress: 0,
    clients: 0,
};

// Covers: a client that finds a daemon older than itself that nobody is
// using stops it and starts its own in its place, then connects to that
// one, instead of failing on a daemon it could replace at no cost (#1776).
// Owner: daemon auto-spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unused_older_daemon_is_replaced_by_the_clients_own() {
    let root = TempDir::new().unwrap();
    let paths = DaemonPaths::resolve(root.path(), |_| None).unwrap();
    let spawner = InProcessSpawner::default();
    let older = RefusingDaemon::start(&paths, &[older_daemon(Some(UNUSED))]);

    let pid = tokio::time::timeout(
        RACE_DEADLINE,
        connect_or_spawn(&paths, ConnectAs::Command.hello(), &spawner),
    )
    .await
    .expect("the client connects once the older daemon stopped")
    .map(|connection| connection.hello.daemon.pid)
    .unwrap();
    tokio::time::timeout(RACE_DEADLINE, older.stopped)
        .await
        .expect("the older daemon was asked to stop")
        .unwrap();

    assert_eq!(
        (pid, spawner.spawns.load(Ordering::SeqCst)),
        (std::process::id(), 1)
    );
    spawner.shutdown.request(Ending::Stop, |_| {});
    let servers = std::mem::take(&mut *spawner.servers.lock().unwrap());
    for served in servers {
        served.await.unwrap().unwrap();
    }
}

/// A spawner whose daemon could not start, and which is never asked to.
struct CannotStart;

impl DaemonSpawner for CannotStart {
    fn check(&self) -> io::Result<()> {
        Err(io::Error::other("its environment is invalid"))
    }

    fn spawn<'a>(&'a self, _: &'a DaemonPaths) -> SpawnFuture<'a> {
        panic!("a daemon that cannot start is not started");
    }
}

// Covers: a client stops no daemon it may not replace (#1776). One with
// work in progress, one another client is connected to, one that does not
// say what a restart costs, and one newer than the client all keep running,
// as does one that came into use, or was itself replaced by a newer one,
// while the client waited to replace it, and one the client could not start
// a daemon in place of. The client starts no daemon, and its error names
// what stood in the way.
// Owner: daemon auto-spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_the_client_may_not_replace_keeps_running() {
    let cost = |in_progress, clients| {
        Some(RestartCost {
            in_progress,
            clients,
        })
    };
    let newer_daemon = VersionMismatch {
        daemon: ProtocolVersion::new(PROTOCOL_VERSION.major + 1, 0),
        client: PROTOCOL_VERSION,
        fix: VersionFix::UpdateClient,
        restart: None,
    };
    let unused = older_daemon(Some(UNUSED));
    let busy = older_daemon(cost(2, 0));
    let refused_by = |refusal: VersionMismatch| refusal.to_string();
    let can_start: &dyn DaemonSpawner = &InProcessSpawner::default();
    let cases: [(&str, Vec<VersionMismatch>, &dyn DaemonSpawner, String); 7] = [
        ("busy", vec![busy], can_start, refused_by(busy)),
        (
            "another client is connected",
            vec![older_daemon(cost(0, 1))],
            can_start,
            refused_by(older_daemon(cost(0, 1))),
        ),
        (
            "does not say what a restart costs",
            vec![older_daemon(None)],
            can_start,
            refused_by(older_daemon(None)),
        ),
        (
            "newer",
            vec![newer_daemon],
            can_start,
            refused_by(newer_daemon),
        ),
        (
            "came into use while the client waited",
            vec![unused, busy],
            can_start,
            refused_by(busy),
        ),
        (
            "replaced by a newer daemon while the client waited",
            vec![unused, newer_daemon],
            can_start,
            refused_by(newer_daemon),
        ),
        (
            "the client's own daemon could not start",
            vec![unused],
            &CannotStart,
            "could not start the rho daemon: its environment is invalid".to_owned(),
        ),
    ];
    for (name, refusals, spawner, expected) in cases {
        let root = TempDir::new().unwrap();
        let paths = DaemonPaths::resolve(root.path(), |_| None).unwrap();
        let _daemon = RefusingDaemon::start(&paths, &refusals);

        let error = tokio::time::timeout(
            RACE_DEADLINE,
            connect_or_spawn(&paths, ConnectAs::Command.hello(), spawner),
        )
        .await
        .expect("the client is refused")
        .err()
        .expect("the client does not connect");

        // A client that stopped the daemon waited for it to let go of the
        // lock, so a lock still held is a daemon nobody signalled.
        assert_eq!(
            (error.to_string(), lock::is_held(&paths).unwrap()),
            (expected, true),
            "{name}"
        );
    }
}
