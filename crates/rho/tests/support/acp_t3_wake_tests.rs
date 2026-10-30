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

/// How the watcher's prompt stopped.
#[derive(Clone, Copy, Debug)]
enum Stop {
    /// T3 Code restarts and kills the process.
    Kill,
    /// T3 Code restarts and closes the process's stdin.
    Quit,
    /// T3 Code loads the live session again in the same process.
    Reload,
}

/// What the watcher's parent did meanwhile.
#[derive(Clone, Copy, Debug)]
enum Parent {
    /// Survived, and its work finished after the stop.
    Signalled,
    /// Ended with the stop, releasing its lock.
    Ended,
    /// Ended as the stop began, so the watcher answered before it was
    /// stopped, too late for T3 Code to see.
    EndedFirst,
}

const PARENT_ENDED: &str =
    "the Rho session that started this background work ended before it finished";

/// A `rho acp --t3code` server with one watcher session waiting on
/// [`TOKEN`], whose parent lock this holds.
struct Watcher {
    workspace: Workspace,
    server: AcpServer,
    session_id: String,
    parent_lock: Option<File>,
}

impl Watcher {
    fn start() -> Self {
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
        let watcher = Self {
            workspace,
            server,
            session_id,
            parent_lock: Some(parent_lock),
        };
        wait_for(&watcher.record());
        watcher
    }

    fn signals(&self) -> std::path::PathBuf {
        self.workspace.home.path().join(".rho/acp-wakes")
    }

    /// The watcher's wait record, published whole once its wait starts.
    fn record(&self) -> std::path::PathBuf {
        self.signals().join(format!("{}.watcher", self.session_id))
    }

    /// Sends `method` while the watcher's prompt is open, and waits for its
    /// reply, skipping the prompt's own.
    fn request_beside_wake(&mut self, method: &str, params: Value) -> Value {
        self.server
            .send(&json!({"jsonrpc": "2.0", "id": method, "method": method, "params": params}));
        loop {
            let message = self.server.recv(&format!("reply to {method}"));
            if message["id"] == method {
                return message;
            }
        }
    }

    fn load(&mut self) {
        let session_id = self.session_id.clone();
        let project = self.workspace.project.clone();
        let loaded = self.request_beside_wake(
            "session/load",
            json!({"sessionId": session_id, "cwd": project, "mcpServers": []}),
        );
        assert!(loaded.get("error").is_none(), "{loaded}");
    }

    /// Sends T3 Code's restart continuation and returns the stop reason and
    /// the turn's updates, without command menus or message ids.
    fn continue_prompt(&mut self) -> (Value, Vec<Value>) {
        let (reply, updates) = self.server.request(
            "session/prompt",
            json!({
                "sessionId": self.session_id,
                "prompt": [{"type": "text", "text": "Continue where you left off."}],
            }),
        );
        let turn = updates
            .into_iter()
            .filter(|update| update["sessionUpdate"] != "available_commands_update")
            .map(|update| {
                json!({"sessionUpdate": update["sessionUpdate"], "content": update["content"]})
            })
            .collect();
        (reply.unwrap()["stopReason"].clone(), turn)
    }
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

// Covers: a T3 Code restart stops a watcher mid-wait, or just after it
// answered unseen, or T3 reloads its live session; T3 then sends "Continue
// where you left off.". The watcher must resume its wait, or repeat its
// answer, with no model turn: its parent's result, or the parent-ended notice
// when the parent died too. A model turn there wakes a model in the watcher
// and hands its parent a bogus result.
// Owner: ACP T3 wake watcher, over the real stdio server
#[test]
fn stopped_watcher_resumes_its_wait_without_a_model_turn() {
    for (stop, parent) in [
        (Stop::Kill, Parent::Signalled),
        (Stop::Quit, Parent::Ended),
        (Stop::Quit, Parent::EndedFirst),
        (Stop::Reload, Parent::Signalled),
    ] {
        let mut watcher = Watcher::start();
        if let Parent::EndedFirst = parent {
            drop(watcher.parent_lock.take());
            while watcher.server.recv("the watcher's answer")["id"] != "wake" {}
        }
        match stop {
            Stop::Kill | Stop::Quit => {
                let restarted = t3_server(&watcher.workspace);
                let stopped = std::mem::replace(&mut watcher.server, restarted);
                match stop {
                    Stop::Quit => stopped.quit(),
                    _ => drop(stopped),
                }
            }
            Stop::Reload => {}
        }
        watcher.load();
        let expected = match parent {
            Parent::Signalled => {
                std::fs::write(watcher.signals().join(TOKEN), "cargo test exited 0").unwrap();
                "cargo test exited 0"
            }
            Parent::Ended | Parent::EndedFirst => {
                drop(watcher.parent_lock.take());
                PARENT_ENDED
            }
        };

        assert_eq!(
            watcher.continue_prompt(),
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

// Covers: `session/close` on a waiting watcher, as T3 Code's task_cancel
// sends, ends its wait for good: a later load and prompt of the session runs
// as an ordinary turn instead of reviving the cancelled wait.
// Owner: ACP T3 wake watcher, over the real stdio server
#[test]
fn closed_watcher_drops_its_wait() {
    let mut watcher = Watcher::start();
    let session_id = watcher.session_id.clone();
    let closed = watcher.request_beside_wake("session/close", json!({"sessionId": session_id}));
    assert!(closed.get("error").is_none(), "{closed}");
    watcher.load();

    let (stop_reason, turn) = watcher.continue_prompt();

    assert_eq!(
        (
            stop_reason,
            watcher.record().exists(),
            turn.iter()
                .any(|update| update["sessionUpdate"] == "tool_call"),
        ),
        (json!("end_turn"), false, true)
    );
}
