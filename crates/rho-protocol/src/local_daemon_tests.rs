use std::{fs, os::unix::fs::PermissionsExt, process::Command, sync::Arc};

use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::net::UnixListener;

use super::*;
use crate::{
    serve_connection, BuildId, ClientInfo, CredentialStoreKind, CredentialStoreStatus, DaemonInfo,
    DaemonStatus, JobListResponse, ProtocolVersion, RestartCost, Service, SettingsService,
    StoreSelectedBy, PROTOCOL_VERSION,
};

fn hello() -> ClientHello {
    ClientHello::frontend(ClientInfo {
        name: "test".into(),
        version: "0".into(),
    })
}

/// A daemon that answers the handshake and nothing else it need not.
struct HandshakeOnly;

impl Service for HandshakeOnly {
    fn refused_newer(&self, _: ProtocolVersion) -> RestartCost {
        RestartCost {
            in_progress: 0,
            clients: 0,
        }
    }

    fn skewed(&self, _: &ClientInfo, _: &BuildId) -> RestartCost {
        RestartCost {
            in_progress: 0,
            clients: 0,
        }
    }

    async fn status(&self) -> DaemonStatus {
        DaemonStatus {
            pid: 7,
            version: "test".into(),
            protocol: PROTOCOL_VERSION,
            started_at: 0,
            uptime_secs: 0,
            jobs: 0,
            in_progress: 0,
            socket: "/test/daemon.sock".into(),
            credential_store: CredentialStoreStatus {
                store: CredentialStoreKind::File,
                selected_by: StoreSelectedBy::Config,
                unreachable: None,
            },
            upgrade: None,
        }
    }

    async fn stop(&self) {}

    async fn restart(&self) {}

    async fn list_jobs(&self) -> JobListResponse {
        JobListResponse { jobs: Vec::new() }
    }
}

impl SettingsService for HandshakeOnly {}

/// A `rho` that must never run.
fn unreachable_rho(root: &Path) -> Command {
    Command::new(root.join("no-such-rho"))
}

// Covers: a daemon listening where its home's endpoint is gets connected to,
// and no `rho` runs.
// Owner: LocalDaemon discovery.
#[tokio::test]
async fn a_running_daemon_is_found_without_starting_one() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    fs::create_dir(&home).unwrap();
    let endpoint = DaemonEndpoint::locate(&home.canonicalize().unwrap(), None).unwrap();
    fs::create_dir(&endpoint.runtime_dir).unwrap();
    let listener = UnixListener::bind(&endpoint.socket).unwrap();
    let info = DaemonInfo {
        version: "test".into(),
        pid: 7,
        build: BuildId::of_this_crate(),
    };
    let served = info.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        serve_connection(
            stream,
            &served,
            Arc::new(HandshakeOnly),
            std::future::pending(),
        )
        .await
    });

    let daemon = LocalDaemon::new(&home).with_xdg_runtime_dir(None);
    let (_client, _notifications, answered) = daemon
        .connect_or_start(unreachable_rho(root.path()), hello())
        .await
        .unwrap();

    assert_eq!(answered.daemon, info);
}

// Covers: with no home, or only the socket a dead daemon left, nothing
// runs.
// Owner: LocalDaemon discovery.
#[tokio::test]
async fn no_daemon_runs_without_a_home_or_behind_a_stale_socket() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    let daemon = LocalDaemon::new(&home).with_xdg_runtime_dir(None);
    assert!(daemon.connect(hello()).await.unwrap().is_none(), "no home");

    fs::create_dir(&home).unwrap();
    let endpoint = daemon.endpoint().unwrap().expect("the home exists");
    fs::create_dir(&endpoint.runtime_dir).unwrap();
    drop(UnixListener::bind(&endpoint.socket).unwrap());
    assert!(endpoint.socket.exists());
    assert!(
        daemon.connect(hello()).await.unwrap().is_none(),
        "stale socket"
    );
}

// Covers: with no daemon running, `connect_or_start` runs `rho daemon
// start` for its home, with the XDG_RUNTIME_DIR it looks under, and reports
// what a failing start said.
// Owner: LocalDaemon start.
#[tokio::test]
async fn a_stopped_daemon_is_started_with_rho_daemon_start() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    let xdg = root.path().join("xdg");
    let record = root.path().join("record");
    let rho = root.path().join("rho");
    fs::write(
        &rho,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" \"$RHO_HOME\" \"$XDG_RUNTIME_DIR\" > {}\necho 'daemon broke' >&2\nexit 3\n",
            record.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&rho, fs::Permissions::from_mode(0o755)).unwrap();

    let started = LocalDaemon::new(&home)
        .with_xdg_runtime_dir(Some(xdg.clone()))
        .connect_or_start(Command::new(&rho), hello())
        .await;

    let Err(LocalDaemonError::Start { stderr, status, .. }) = started else {
        panic!("expected a failed start, got {:?}", started.err());
    };
    assert_eq!((stderr.as_str(), status.code()), ("daemon broke", Some(3)));
    assert_eq!(
        fs::read_to_string(&record).unwrap(),
        format!("daemon start\n{}\n{}\n", home.display(), xdg.display())
    );
}
