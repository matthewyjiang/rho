//! The processes a daemon session's tools start, through the real binary:
//! the daemon's stop ends them, as closing a session ends them in every
//! rho host (#1661).

use std::path::Path;

use pretty_assertions::assert_eq;

use rho_protocol::{
    methods::{SessionCancel, SessionPrompt},
    Client, SessionCancelParams, SessionId, SessionPromptParams,
};
use serde_json::json;

use super::sessions::{new_session, settled_job, Sandbox};

/// A command whose shell starts a child that sleeps until something ends
/// it, records the child's pid in `file` in the workspace, and waits for it.
/// The child is the tool's grandchild, so only ending the tool's whole
/// process group ends it.
fn sleeper(file: &str) -> String {
    format!("sleep 600 & echo $! > {file} && wait")
}

/// Prompts `session` with the tool call `call`, which the `tool-call`
/// fixture makes.
async fn prompt_call(client: &Client, session: &SessionId, call: serde_json::Value) {
    client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            text: call.to_string(),
            timeout_ms: None,
        })
        .await
        .unwrap();
}

/// The pid a [`sleeper`] recorded in `file` under `workspace`, once it has.
fn recorded_pid(workspace: &Path, file: &str) -> Option<libc::pid_t> {
    std::fs::read_to_string(workspace.join(file))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Whether process `pid` exists.
fn exists(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks that `pid` exists; nothing is sent.
    unsafe { libc::kill(pid, 0) == 0 }
}

// Covers: `rho daemon stop` ends the processes its sessions' tools started,
// children included: the command a turn under way runs, and a background
// process that a turn started and that outlived the turn's cancel (#1661).
// Owner: daemon stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemons_stop_ends_the_processes_its_sessions_started() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let (_daemon, client) = sandbox.start_daemon("tool-call").await;
    let running = new_session(&client, &sandbox.workspace).await;
    let bash = json!({
        "id": "foreground",
        "name": "bash",
        "arguments": {"command": sleeper("foreground.pid")},
    });
    prompt_call(&client, &running, bash).await;
    let cancelled = new_session(&client, &sandbox.workspace).await;
    let process = json!({
        "id": "background",
        "name": "process",
        "arguments": {"action": "start", "command": sleeper("background.pid")},
    });
    prompt_call(&client, &cancelled, process).await;
    let pids = sandbox
        .poll_until("both processes to start", async || {
            Some([
                recorded_pid(&sandbox.workspace, "foreground.pid")?,
                recorded_pid(&sandbox.workspace, "background.pid")?,
            ])
        })
        .await;
    client
        .request::<SessionCancel>(&SessionCancelParams {
            session: cancelled.clone(),
        })
        .await
        .unwrap();
    settled_job(&sandbox, &client, &cancelled).await;
    let before_stop = pids.map(exists);

    let stop = sandbox
        .rho("tool-call")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    sandbox
        .poll_until("the processes to end", async || {
            (!pids.into_iter().any(exists)).then_some(())
        })
        .await;

    assert!(stop.success(), "log:\n{}", sandbox.daemon_log());
    assert_eq!(before_stop, [true, true]);
}
