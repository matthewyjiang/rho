//! The version-skew policy at the handshake (#1776), through the real
//! binary: what the daemon answers a client of another rho build, and of
//! another protocol version, at `rho/hello`, and what `rho daemon restart`
//! does about a daemon of another build.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::HELLO, BuildId, BuildSkew, DaemonInfo, HelloResponse, Message, ProtocolVersion,
    Response, RestartCost, VersionFix, VersionMismatch, PROTOCOL_VERSION,
};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::{
    sessions::{settled_job, Sandbox},
    upgrade::{blocked_on, fifo, open_gate},
};

/// What the sandbox's daemon answers the hello of a client that speaks
/// `protocol` and names `build`, sent as a line on a new connection.
async fn hello(
    sandbox: &Sandbox,
    protocol: ProtocolVersion,
    build: &BuildId,
) -> Result<HelloResponse, VersionMismatch> {
    let stream = sandbox.stream().await.expect("the daemon runs");
    let (read, mut write) = tokio::io::split(stream);
    let hello = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": HELLO,
        // Not the daemon's rho version: the builds differ.
        "params": {
            "protocol": protocol,
            "client": {"name": "another-build", "version": "999.0.0"},
            "role": "frontend",
            "build": build,
        },
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

// Covers: the daemon refuses a client of a newer protocol version at the
// handshake, before any request can fail on it, and accepts one of its own
// version but another build with a warning. Both name what a daemon
// restart would cost as the daemon stands: the turn under way and the
// client connected, then that client alone once the turn ended, then
// nothing once it left. A client of an older version is told to change
// itself, and one of the daemon's own build hears nothing of builds.
// Owner: daemon process (handshake).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_of_another_version_or_build_hears_at_hello_what_a_restart_costs() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let (daemon, client) = sandbox.spawn_daemon(sandbox.rho("tool-call"), &[]).await;
    let session = sandbox.new_session(&client).await;
    let release = fifo(sandbox.workspace.join("release.fifo"));
    let newer = ProtocolVersion::new(PROTOCOL_VERSION.major, PROTOCOL_VERSION.minor + 1);
    let older = ProtocolVersion::new(0, 0);
    // The test links the daemon's protocol crate, so its build is the
    // daemon's.
    let daemon_build = BuildId::of_this_crate();
    let another_build = BuildId::new("another build");
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
    // The accepted hello last: the daemon counts its client until it
    // notices it left.
    let during_the_turn = (
        hello(&sandbox, newer, &another_build).await,
        hello(&sandbox, older, &another_build).await,
        hello(&sandbox, PROTOCOL_VERSION, &another_build).await,
    );
    open_gate(&sandbox, &release).await;
    settled_job(&sandbox, &client, &session).await;
    // The job reads idle before the ended turn lets go of the session's
    // runtime, which counts as work under way until then, and the daemon
    // notices each client leave on its own time.
    let after_the_turn = restart_costs(0, 1);
    sandbox
        .poll_until("the ended turn to let go of the session", async || {
            (hello(&sandbox, newer, &another_build).await == after_the_turn).then_some(())
        })
        .await;
    let same_build = hello(&sandbox, PROTOCOL_VERSION, &daemon_build).await;
    drop(client);
    let unused = restart_costs(0, 0);
    sandbox
        .poll_until("the daemon to see its clients leave", async || {
            (hello(&sandbox, newer, &another_build).await == unused).then_some(())
        })
        .await;
    let accepted = |skew| {
        Ok(HelloResponse {
            protocol: PROTOCOL_VERSION,
            daemon: DaemonInfo {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                pid: daemon.id(),
                build: daemon_build.clone(),
            },
            skew,
        })
    };

    assert_eq!(
        (during_the_turn, same_build),
        (
            (
                restart_costs(1, 1),
                Err(VersionMismatch {
                    daemon: PROTOCOL_VERSION,
                    client: older,
                    fix: VersionFix::UpdateClient,
                    restart: None,
                }),
                accepted(Some(BuildSkew {
                    restart: RestartCost {
                        in_progress: 1,
                        clients: 1,
                    },
                })),
            ),
            accepted(None),
        ),
        "daemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: `rho daemon restart` re-executes the binary the daemon started
// from, which may be the other build again; the restart still ends with a
// daemon of the build it was run from, so the advice of a skewed hello
// works.
// Owner: rho daemon restart CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_replaces_a_daemon_of_another_build_with_its_own() {
    let sandbox = Sandbox::new();
    let mut another_build = sandbox.rho("fixed");
    another_build.env("RHO_TUI_TEST_BUILD_ID", "another build");
    drop(sandbox.spawn_daemon(another_build, &[]).await.1);

    let restarted = sandbox
        .rho("fixed")
        .args(["daemon", "restart"])
        .status()
        .unwrap();
    let daemon_build = BuildId::of_this_crate();
    let answer = hello(&sandbox, PROTOCOL_VERSION, &daemon_build)
        .await
        .map(|hello| (hello.daemon.build, hello.skew));

    assert_eq!(
        (restarted.success(), answer),
        (true, Ok((daemon_build, None))),
        "daemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
