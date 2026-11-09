//! Re-executing the daemon in place (#1713), through the real binary: a
//! replaced executable is re-executed once the daemon is idle, never during
//! a turn, and the daemon keeps its pid, lock, and socket, so a client that
//! connects meanwhile waits for it; `rho daemon restart` interrupts the
//! turns under way, after which their sessions take prompts again.

use std::{
    ffi::CString,
    fs,
    io::Write,
    os::unix::{
        ffi::OsStrExt,
        fs::{symlink, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{SessionAttach, SessionCancel, Status},
    Client, ClientInfo, Cursor, DaemonNotification, DetachReason, JobState, Notifications,
    PendingUpgrade, Replay, SessionAttachParams, SessionAttachResponse, SessionCancelParams,
    SessionEvent, SessionId, SessionSetup, StatusParams,
};
use serde_json::json;
use tempfile::TempDir;
use tokio::net::UnixStream;

use super::sessions::{settled_job, Sandbox, DEADLINE};

/// `rho` installed at a path a test replaces, as an upgrade does: at first
/// a symlink to the binary under test.
struct Install {
    dir: TempDir,
    path: PathBuf,
}

impl Install {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rho");
        symlink(env!("CARGO_BIN_EXE_rho"), &path).unwrap();
        Self { dir, path }
    }

    /// Puts a new executable at the installed path, by a rename as an
    /// installer does: a script that runs the binary under test. A daemon
    /// it starts first waits until `gate`, a FIFO, is opened for writing,
    /// when given, so the test acts while the daemon re-executes.
    fn upgrade(&self, gate: Option<&Path>) {
        let wait = gate
            .map(|gate| {
                format!(
                    "if [ \"$1\" = daemon ]; then read _ < '{}'; fi\n",
                    gate.display()
                )
            })
            .unwrap_or_default();
        let script = format!(
            "#!/bin/sh\n{wait}exec '{}' \"$@\"\n",
            env!("CARGO_BIN_EXE_rho")
        );
        let staged = self.dir.path().join("rho.new");
        fs::write(&staged, script).unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&staged, &self.path).unwrap();
    }
}

/// Makes a FIFO at `path`: opening it blocks until the other end opens.
fn fifo(path: PathBuf) -> PathBuf {
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `name` is a NUL-terminated path that outlives the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    path
}

/// Opens FIFO `gate` for writing once something opened it to read, and
/// writes a line, which lets that reader go on.
async fn open_gate(sandbox: &Sandbox, gate: &Path) {
    // Without a reader, a non-blocking open fails at once with ENXIO.
    let mut writer = sandbox
        .poll_until("something to wait on the gate", async || {
            fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(gate)
                .ok()
        })
        .await;
    writer.write_all(b"go\n").unwrap();
}

/// Everything the daemon sends on a connection until it closes it.
async fn until_closed(notifications: &mut Notifications) -> Vec<DaemonNotification> {
    let mut heard = Vec::new();
    while let Some(notification) = tokio::time::timeout(DEADLINE, notifications.recv())
        .await
        .expect("the daemon closes the connection")
    {
        heard.push(notification);
    }
    heard
}

/// What a client following a session saw of its job: each state, and why
/// it was detached.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    State(JobState),
    Detached(DetachReason),
}

fn seen(heard: &[DaemonNotification]) -> Vec<Seen> {
    heard
        .iter()
        .filter_map(|notification| match notification {
            DaemonNotification::SessionEvent(params) => match &params.event {
                SessionEvent::State { state, .. } => Some(Seen::State(*state)),
                _ => None,
            },
            DaemonNotification::SessionDetached(params) => Some(Seen::Detached(params.reason)),
            _ => None,
        })
        .collect()
}

/// Where a client that heard `heard` after attaching as `attached` is in
/// the session's stream.
fn cursor(attached: &SessionAttachResponse, heard: &[DaemonNotification]) -> Option<Cursor> {
    heard
        .iter()
        .rev()
        .find_map(|notification| match notification {
            DaemonNotification::SessionEvent(params) => Some(Cursor {
                stream: attached.stream.clone(),
                seq: params.seq,
            }),
            _ => None,
        })
}

async fn attach(
    sandbox: &Sandbox,
    client: &Client,
    session: &SessionId,
    after: Option<Cursor>,
) -> SessionAttachResponse {
    client
        .request::<SessionAttach>(&SessionAttachParams {
            session: session.clone(),
            after,
            environment: sandbox.client_environment(),
            setup: SessionSetup::default(),
        })
        .await
        .unwrap()
}

/// A bash call that blocks until FIFO `gate`, in the workspace, opens.
fn blocked_on(gate: &str) -> serde_json::Value {
    json!({
        "id": "blocked",
        "name": "bash",
        "arguments": {"command": format!("read _ < {gate}")},
    })
}

// Covers: a daemon whose executable is replaced during a turn reports the
// upgrade pending and waiting for that turn, lets the turn run to its end,
// and only then re-executes: a client following the session sees the turn
// complete before it is detached as restarting, and reattaching with its
// cursor reaches the same pid, which resets it to the completed turn.
// Owner: daemon upgrade in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_daemon_upgrades_only_once_the_running_turn_ends() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let install = Install::new();
    let (daemon, client) = sandbox
        .spawn_daemon(sandbox.rho_at(&install.path, "tool-call"), &[])
        .await;
    let session = sandbox.new_session(&client).await;
    let release = fifo(sandbox.workspace.join("release.fifo"));
    let (follower, mut notifications) = sandbox.connect_following().await;
    let attached = attach(&sandbox, &follower, &session, None).await;
    sandbox
        .prompt_tool_call(&client, &session, &blocked_on("release.fifo"))
        .await;

    install.upgrade(None);
    let pending = pending_upgrade(&sandbox, &client).await;
    open_gate(&sandbox, &release).await;
    let heard = until_closed(&mut notifications).await;
    // Waits while the daemon re-executes.
    let reconnected = sandbox.connect().await;
    let status = reconnected
        .request::<Status>(&StatusParams {})
        .await
        .unwrap();
    let reattached = attach(&sandbox, &reconnected, &session, cursor(&attached, &heard)).await;

    assert_eq!(
        (
            pending,
            seen(&heard),
            (status.pid, status.upgrade),
            (reattached.replay, reattached.state),
        ),
        (
            PendingUpgrade {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                waiting_for: 1,
            },
            vec![
                Seen::State(JobState::Running),
                Seen::State(JobState::Completed),
                Seen::Detached(DetachReason::Restarting),
            ],
            (daemon.id(), None),
            (Replay::Reset, JobState::Completed),
        ),
        "daemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

/// Polls `rho/daemon/status` until the upgrade is pending, and returns it.
async fn pending_upgrade(sandbox: &Sandbox, client: &Client) -> PendingUpgrade {
    sandbox
        .poll_until("the upgrade to be pending", async || {
            client
                .request::<Status>(&StatusParams {})
                .await
                .unwrap()
                .upgrade
        })
        .await
}

// Covers: a replaced daemon with no turn under way still waits for a
// background process a session's turn started and outlived, which the
// restart would kill, and upgrades once it ended and its result was taken
// by the next prompt.
// Owner: daemon upgrade in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upgrade_waits_for_a_sessions_background_work() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let install = Install::new();
    let (_daemon, client) = sandbox
        .spawn_daemon(sandbox.rho_at(&install.path, "tool-call"), &[])
        .await;
    let session = sandbox.new_session(&client).await;
    let release = fifo(sandbox.workspace.join("background.fifo"));
    let background = json!({
        "id": "background",
        "name": "process",
        "arguments": {
            "action": "start",
            "command": "touch started && read _ < background.fifo",
        },
    });
    sandbox
        .prompt_tool_call(&client, &session, &background)
        .await;
    sandbox
        .poll_until("the background process to start", async || {
            sandbox.workspace.join("started").exists().then_some(())
        })
        .await;
    // A turn waits for the processes it started, so only its cancel lets
    // one outlive it.
    client
        .request::<SessionCancel>(&SessionCancelParams {
            session: session.clone(),
        })
        .await
        .unwrap();
    let cancelled = settled_job(&sandbox, &client, &session).await.state;
    let (_watcher, mut notifications) = sandbox.connect_following().await;

    install.upgrade(None);
    let pending = pending_upgrade(&sandbox, &client).await;
    open_gate(&sandbox, &release).await;
    let prompted = sandbox.prompt(&client, &session).await;
    until_closed(&mut notifications).await;

    assert_eq!(
        (cancelled, pending.waiting_for, prompted),
        (JobState::Cancelled, 1, Ok(())),
        "daemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a replaced daemon waits while a client follows a session that
// has saved nothing yet, which a restarted daemon could not reopen for it,
// and upgrades once that client leaves.
// Owner: daemon upgrade in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upgrade_waits_while_a_client_follows_an_unsaved_session() {
    let sandbox = Sandbox::new();
    let install = Install::new();
    let (_daemon, client) = sandbox
        .spawn_daemon(sandbox.rho_at(&install.path, "fixed"), &[])
        .await;
    let session = sandbox.new_session(&client).await;
    let (follower, _events) = sandbox.connect_following().await;
    attach(&sandbox, &follower, &session, None).await;
    let (_watcher, mut notifications) = sandbox.connect_following().await;

    install.upgrade(None);
    let pending = pending_upgrade(&sandbox, &client).await;
    drop(follower);
    until_closed(&mut notifications).await;

    assert_eq!(
        pending.waiting_for,
        1,
        "daemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: an idle daemon whose executable is replaced re-executes it as the
// same process and keeps its socket: the connections it had are closed, and
// a client that connects while the new binary starts waits in the socket's
// backlog until that binary answers, instead of failing, with no upgrade
// left pending.
// Owner: daemon upgrade in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_daemon_upgrades_in_place_while_a_new_client_waits() {
    let sandbox = Sandbox::new();
    let install = Install::new();
    let (daemon, _client) = sandbox
        .spawn_daemon(sandbox.rho_at(&install.path, "fixed"), &[])
        .await;
    let (_before, mut notifications) = sandbox.connect_following().await;
    let gate = fifo(install.dir.path().join("gate"));

    install.upgrade(Some(&gate));
    until_closed(&mut notifications).await;
    // The new binary waits on the gate, so this connection lands in the
    // socket's backlog before anything can accept it.
    let stream = UnixStream::connect(sandbox.isolated.daemon_socket().unwrap())
        .await
        .expect("the restarting daemon keeps its socket");
    let waiting = tokio::spawn(Client::handshake(
        stream,
        ClientInfo {
            name: "daemon-upgrade-test".into(),
            version: "0".into(),
        },
    ));
    open_gate(&sandbox, &gate).await;
    let (client, _, hello) = tokio::time::timeout(DEADLINE, waiting)
        .await
        .expect("the upgraded daemon answers the waiting client")
        .unwrap()
        .unwrap();
    let status = client.request::<Status>(&StatusParams {}).await.unwrap();

    assert_eq!(
        (hello.daemon.pid, status.upgrade),
        (daemon.id(), None),
        "daemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: `rho daemon restart` during a turn re-executes the daemon as the
// same process: the client following the session is detached as
// restarting, the turn is recorded interrupted, and the session takes a
// prompt again, which completes.
// Owner: daemon restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_interrupts_the_running_turn_and_the_session_takes_prompts_again() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let (daemon, client) = sandbox.spawn_daemon(sandbox.rho("tool-call"), &[]).await;
    let session = sandbox.new_session(&client).await;
    fifo(sandbox.workspace.join("never.fifo"));
    let (follower, mut notifications) = sandbox.connect_following().await;
    attach(&sandbox, &follower, &session, None).await;
    sandbox
        .prompt_tool_call(&client, &session, &blocked_on("never.fifo"))
        .await;

    let restarted = sandbox
        .rho("tool-call")
        .args(["daemon", "restart"])
        .status()
        .unwrap();
    let heard = until_closed(&mut notifications).await;
    let client = sandbox.connect().await;
    let interrupted = settled_job(&sandbox, &client, &session).await.state;
    let prompted = sandbox.prompt(&client, &session).await;
    let completed = settled_job(&sandbox, &client, &session).await.state;
    let status = client.request::<Status>(&StatusParams {}).await.unwrap();

    assert_eq!(
        (
            restarted.success(),
            seen(&heard).last(),
            interrupted,
            prompted,
            completed,
            status.pid,
        ),
        (
            true,
            Some(&Seen::Detached(DetachReason::Restarting)),
            JobState::Interrupted,
            Ok(()),
            JobState::Completed,
            daemon.id(),
        ),
        "daemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
