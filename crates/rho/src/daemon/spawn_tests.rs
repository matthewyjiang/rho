use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use rho_sdk::CancellationToken;
use tempfile::TempDir;
use tokio::task::JoinHandle;

use super::*;
use crate::daemon::{client::cli_client_info, lock, server};

/// Failure bound for every racing caller to connect.
const RACE_DEADLINE: Duration = Duration::from_secs(30);

/// Starts the daemon inside the test process and counts how often it is asked.
struct InProcessSpawner {
    spawns: AtomicUsize,
    shutdown: CancellationToken,
    servers: Mutex<Vec<JoinHandle<io::Result<()>>>>,
}

impl DaemonSpawner for InProcessSpawner {
    fn spawn(&self, paths: &DaemonPaths) -> io::Result<()> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        let lock = lock::acquire(paths).map_err(io::Error::other)?;
        let bound = server::bind(paths, lock)?;
        let served = tokio::spawn(bound.serve(self.shutdown.clone()));
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
        shutdown: CancellationToken::new(),
        servers: Mutex::new(Vec::new()),
    });

    let callers: Vec<_> = (0..8)
        .map(|_| {
            let paths = paths.clone();
            let spawner = Arc::clone(&spawner);
            tokio::spawn(async move {
                connect_or_spawn(&paths, cli_client_info(), spawner.as_ref())
                    .await
                    .map(|(_, hello)| hello.daemon.pid)
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
    spawner.shutdown.cancel();
    let servers = std::mem::take(&mut *spawner.servers.lock().unwrap());
    for served in servers {
        served.await.unwrap().unwrap();
    }
}
