//! T3 Code wake watchers (`rho acp --t3code`) over the real stdio server.

use std::{
    fs::File,
    path::Path,
    time::{Duration, Instant},
};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace, REPLY_TIMEOUT};

const TOKEN: &str = "0f4e2c3a-9b1d-4e5f-8a7b-6c5d4e3f2a1b";

/// How T3 Code's restart stopped the watcher's process.
#[derive(Clone, Copy, Debug)]
enum Stop {
    Kill,
    /// Closed its stdin, so the session shut down.
    Quit,
}

/// What the watcher's parent did across the restart.
#[derive(Clone, Copy, Debug)]
enum Parent {
    /// Survived, and its work finished after the restart.
    Signalled,
    /// Ended with the restart, releasing its lock.
    Ended,
}

fn t3_server(workspace: &Workspace) -> AcpServer {
    let mut command = AcpServer::command(
        workspace.home.path(),
        &workspace.project,
        "read-path",
        &workspace.extra.join("notes.txt"),
    );
    let mut server = AcpServer::from_command(command.arg("--t3code"));
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    server
}

/// Waits until `path` exists, polling: the watcher reports nothing while it
/// waits.
fn wait_for(path: &Path) {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

// Covers: a T3 Code restart stops a watcher mid-wait, then reopens its
// session and sends "Continue where you left off.". The watcher must resume
// its wait with no model turn, and end with its parent's result, or with the
// parent-ended notice when the parent died too. A model turn there wakes a
// model in the watcher and hands its parent a bogus result.
// Owner: ACP T3 wake watcher, over the real stdio server
#[test]
fn restarted_watcher_resumes_its_wait_without_a_model_turn() {
    for (stop, parent) in [(Stop::Kill, Parent::Signalled), (Stop::Quit, Parent::Ended)] {
        let workspace = Workspace::new();
        let signals = workspace.home.path().join(".rho/acp-wakes");
        std::fs::create_dir_all(&signals).unwrap();
        let parent_lock = File::create(signals.join(format!("{TOKEN}.lock"))).unwrap();
        parent_lock.lock().unwrap();

        let mut server = t3_server(&workspace);
        let created = server
            .request(
                "session/new",
                json!({"cwd": workspace.project, "mcpServers": []}),
            )
            .0
            .unwrap();
        let session_id = created["sessionId"].as_str().unwrap().to_owned();
        server.send(&json!({
            "jsonrpc": "2.0",
            "id": "wake",
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": format!("/rho-wake {TOKEN}")}],
            },
        }));
        wait_for(&signals.join(format!("{session_id}.watcher")));
        match stop {
            Stop::Kill => drop(server),
            Stop::Quit => server.quit(),
        }

        let expected = match parent {
            Parent::Signalled => {
                std::fs::write(signals.join(TOKEN), "cargo test exited 0").unwrap();
                "cargo test exited 0"
            }
            Parent::Ended => {
                drop(parent_lock);
                "the Rho session that started this background work ended before it finished"
            }
        };
        let mut server = t3_server(&workspace);
        server
            .request(
                "session/load",
                json!({"sessionId": session_id, "cwd": workspace.project, "mcpServers": []}),
            )
            .0
            .unwrap();
        let (reply, updates) = server.request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "Continue where you left off."}],
            }),
        );

        let turn: Vec<Value> = updates
            .into_iter()
            .filter(|update| update["sessionUpdate"] != "available_commands_update")
            .map(|update| {
                json!({"sessionUpdate": update["sessionUpdate"], "content": update["content"]})
            })
            .collect();
        assert_eq!(
            (reply.unwrap()["stopReason"].clone(), turn),
            (
                json!("end_turn"),
                vec![json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": expected},
                })]
            ),
            "{stop:?}, {parent:?}"
        );
    }
}
