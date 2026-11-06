use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

use pretty_assertions::assert_eq;
use rho_protocol::{methods, StatusParams, StopParams, StopResponse};
use tempfile::TempDir;

use super::*;
use crate::daemon::{
    client, lock,
    test_support::{test_status, StubHost},
};

/// Failure bound for the server task to finish after a stop request.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(10);

fn prepared_paths(root: &Path) -> DaemonPaths {
    let paths = DaemonPaths::resolve(root, |_| None).unwrap();
    paths.prepare_runtime_dir().unwrap();
    paths
}

// Covers: a daemon started over a crashed one's leftover socket serves the
// handshake, routes requests to the service its caller passed, keeps its
// socket private, and when that service stops it on `rho/daemon/stop`,
// acknowledges, ends its serve task, removes its socket and info file, and
// frees the lock.
// Owner: daemon server lifecycle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_serves_status_and_cleans_up_on_stop() {
    let root = TempDir::new().unwrap();
    let paths = prepared_paths(root.path());
    fs::write(&paths.socket, "left by a crashed daemon").unwrap();
    let bound = bind(&paths, lock::acquire(&paths).unwrap()).unwrap();
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(async move {
        bound
            .serve(StubHost::new(&shutdown), shutdown)
            .await
            .release()
    });

    let socket_mode = fs::metadata(&paths.socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(socket_mode, 0o600);
    let (client, hello) = client::connect(&paths, client::cli_client_info())
        .await
        .unwrap();
    assert_eq!(hello.daemon.pid, std::process::id());
    assert_eq!(hello.protocol, PROTOCOL_VERSION);

    let status = client
        .request::<methods::Status>(&StatusParams {})
        .await
        .unwrap();
    assert_eq!(status, test_status());
    assert_eq!(
        lock::read_record(&paths).map(|record| record.pid),
        Some(std::process::id())
    );

    let ack = client
        .request::<methods::Stop>(&StopParams {})
        .await
        .unwrap();
    assert_eq!(ack, StopResponse {});
    tokio::time::timeout(SHUTDOWN_DEADLINE, server)
        .await
        .expect("server stops after rho/daemon/stop")
        .unwrap()
        .unwrap();

    assert!(!paths.socket.exists(), "socket removed");
    assert!(!paths.info_file.exists(), "info file removed");
    assert!(!lock::is_held(&paths).unwrap(), "lock released");
}
