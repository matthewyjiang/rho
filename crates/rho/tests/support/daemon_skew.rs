//! The version-skew policy at the handshake (#1776), through the real
//! binary: what the daemon answers a client of another rho build, and of
//! another protocol version, at `rho/hello`.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::HELLO, DaemonInfo, HelloResponse, Message, ProtocolVersion, Response, RestartCost,
    VersionFix, VersionMismatch, PROTOCOL_VERSION,
};
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

use super::{
    sessions::{settled_job, Sandbox},
    upgrade::{blocked_on, fifo, open_gate},
};

/// What the sandbox's daemon answers the hello of a client that speaks
/// `protocol`, sent as a line on a new connection, as a client this crate
/// was not built into would send it.
async fn hello(
    sandbox: &Sandbox,
    protocol: ProtocolVersion,
) -> Result<HelloResponse, VersionMismatch> {
    let socket = sandbox.isolated.daemon_socket().expect("the daemon runs");
    let (read, mut write) = UnixStream::connect(socket).await.unwrap().into_split();
    let hello = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": HELLO,
        // Not the daemon's rho version: the builds differ.
        "params": {"protocol": protocol, "client": {"name": "another-build", "version": "999.0.0"}},
    });
    write
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .unwrap();
    let answer = BufReader::new(read)
        .lines()
        .next_line()
        .await
        .unwrap()
        .expect("the daemon answers the hello");
    let Message::Response(Response { outcome, .. }) = Message::parse(answer.as_bytes()).unwrap()
    else {
        panic!("expected a response, got {answer}");
    };
    match outcome {
        Ok(accepted) => Ok(serde_json::from_value(accepted).unwrap()),
        Err(refused) => Err(serde_json::from_value(refused.data.expect("the mismatch")).unwrap()),
    }
}

// Covers: the daemon answers a client of another rho build that speaks its
// protocol version, and refuses one of a newer protocol version at the
// handshake, before any request can fail on it. The refusal names a daemon
// restart as the fix and what it would cost as the daemon stands: the turn
// under way and the client connected, then that client alone once the turn
// ended, then nothing once it left. A client of an older version is told to
// change itself.
// Owner: daemon process (handshake).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_newer_client_is_told_at_hello_what_restarting_the_daemon_would_cost() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let (daemon, client) = sandbox.spawn_daemon(sandbox.rho("tool-call"), &[]).await;
    let session = sandbox.new_session(&client).await;
    let release = fifo(sandbox.workspace.join("release.fifo"));
    let newer = ProtocolVersion::new(PROTOCOL_VERSION.major, PROTOCOL_VERSION.minor + 1);
    let older = ProtocolVersion::new(0, 0);
    let restart_costs = |in_progress, clients| {
        Err(VersionMismatch {
            daemon: PROTOCOL_VERSION,
            client: newer,
            fix: VersionFix::RestartDaemon,
            restart: Some(RestartCost {
                in_progress,
                clients,
            }),
        })
    };

    sandbox
        .prompt_tool_call(&client, &session, &blocked_on("release.fifo"))
        .await;
    let during_the_turn = (hello(&sandbox, newer).await, hello(&sandbox, older).await);
    open_gate(&sandbox, &release).await;
    settled_job(&sandbox, &client, &session).await;
    let after_the_turn = hello(&sandbox, newer).await;
    let same_protocol = hello(&sandbox, PROTOCOL_VERSION).await;
    drop(client);
    // The daemon notices each client leave on its own time.
    let unused = restart_costs(0, 0);
    sandbox
        .poll_until("the daemon to see its clients leave", async || {
            (hello(&sandbox, newer).await == unused).then_some(())
        })
        .await;

    assert_eq!(
        (during_the_turn, after_the_turn, same_protocol),
        (
            (
                restart_costs(1, 1),
                Err(VersionMismatch {
                    daemon: PROTOCOL_VERSION,
                    client: older,
                    fix: VersionFix::UpdateClient,
                    restart: None,
                }),
            ),
            restart_costs(0, 1),
            Ok(HelloResponse {
                protocol: PROTOCOL_VERSION,
                daemon: DaemonInfo {
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                    pid: daemon.id(),
                },
                capabilities: Vec::new(),
            }),
        ),
        "daemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
