use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::task::JoinHandle;

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
