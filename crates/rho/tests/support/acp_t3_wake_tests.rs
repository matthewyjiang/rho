//! T3 Code wake watchers (`rho acp --t3code`) over the real ACP v2 stdio
//! server and the daemon behind it.
//!
//! The test plays the watcher's parent: it holds the parent's lock on the
//! signal, writes the signal or lets the lock go, and reads what the
//! watcher session answers. A wait cut off mid-way by a killed `rho acp`
//! goes on in the daemon with no event the new client could wait on, so
//! these cover a wait answered before the restart.

use std::{fs::File, path::PathBuf};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace};

const TOKEN: &str = "0f4e2c3a-9b1d-4e5f-8a7b-6c5d4e3f2a1b";

const PARENT_ENDED: &str =
    "the Rho session that started this background work ended before it finished";

/// What the watcher's parent did while the watcher waited.
#[derive(Clone, Copy, Debug)]
enum Parent {
    /// Its work finished: it wrote the signal.
    Signalled,
    /// It ended, releasing its lock.
    Ended,
}

/// A `rho acp --t3code` server with one watcher session waiting on
/// [`TOKEN`], whose parent lock this holds.
struct Watcher {
    /// Before `workspace`, so the server is killed before the sandbox
    /// tears down (see [`rho_tui_pty::IsolatedHome`]).
    server: AcpServer,
    workspace: Workspace,
    session_id: String,
    parent_lock: Option<File>,
}

impl Watcher {
    /// Opens the watcher session and sends its `/rho-wake` prompt, returning
    /// once the daemon accepted it.
    fn start() -> Self {
        let workspace = Workspace::new();
        let signals = workspace.home.rho_home().join("acp-wakes");
        std::fs::create_dir_all(&signals).unwrap();
        let parent_lock = File::create(signals.join(format!("{TOKEN}.lock"))).unwrap();
        parent_lock.lock().unwrap();
        let mut server = t3_server(&workspace);
        let created = server
            .request("session/new", json!({"cwd": workspace.project}))
            .0
            .unwrap();
        let session_id = created["sessionId"].as_str().unwrap().to_owned();
        let mut watcher = Self {
            workspace,
            server,
            session_id,
            parent_lock: Some(parent_lock),
        };
        let session_id = watcher.session_id.clone();
        let accepted = watcher.request_beside_wake(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": format!("/rho-wake {TOKEN}")}],
            }),
        );
        assert!(accepted.get("error").is_none(), "{accepted}");
        watcher
    }

    fn signals(&self) -> PathBuf {
        self.workspace.home.rho_home().join("acp-wakes")
    }

    /// The watcher session's record of its wait.
    fn record(&self) -> PathBuf {
        self.signals().join(format!("{}.watcher", self.session_id))
    }

    /// Plays the parent: signals or lets go. Returns the watcher's answer.
    fn parent(&mut self, parent: Parent) -> &'static str {
        match parent {
            Parent::Signalled => {
                std::fs::write(self.signals().join(TOKEN), "cargo test exited 0").unwrap();
                "cargo test exited 0"
            }
            Parent::Ended => {
                drop(self.parent_lock.take());
                PARENT_ENDED
            }
        }
    }

    /// Sends `method` while the watcher's prompt may be open, and waits for
    /// its reply, skipping the prompt's own traffic.
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

    /// Sends `method`, which ends the waiting prompt, and waits for both
    /// its reply and the prompt's `idle` with `stop_reason`, which come in
    /// either order.
    fn request_ending_wake(&mut self, method: &str, params: Value, stop_reason: &str) -> Value {
        self.server
            .send(&json!({"jsonrpc": "2.0", "id": method, "method": method, "params": params}));
        let mut reply = None;
        let mut idle = false;
        loop {
            let message = self.server.recv(&format!("reply to {method} and idle"));
            let update = &message["params"]["update"];
            if message["id"] == method {
                reply = Some(message);
            } else if update["sessionUpdate"] == "state_update" && update["state"] == "idle" {
                assert_eq!(update["stopReason"], stop_reason);
                idle = true;
            }
            if let (Some(reply), true) = (&reply, idle) {
                return reply.clone();
            }
        }
    }

    /// Attaches the session again, as T3 Code does on start.
    fn load(&mut self) {
        let session_id = self.session_id.clone();
        let project = self.workspace.project.clone();
        let loaded = self.request_beside_wake(
            "session/resume",
            json!({"sessionId": session_id, "cwd": project, "replayFrom": {"type": "start"}}),
        );
        assert!(loaded.get("error").is_none(), "{loaded}");
    }

    /// The v2 prompt reply is only acceptance; the wait finishes at `idle`
    /// with `stop_reason`. Returns the watcher's answer.
    fn finish_wake(&mut self, stop_reason: &str) -> String {
        let mut answer = String::new();
        loop {
            let message = self.server.recv("the watcher's idle state");
            let update = &message["params"]["update"];
            match update["sessionUpdate"].as_str() {
                Some("agent_message_chunk") => {
                    answer.push_str(update["content"]["text"].as_str().unwrap_or_default());
                }
                Some("state_update") if update["state"] == "idle" => {
                    assert_eq!(update["stopReason"], stop_reason);
                    return answer;
                }
                _ => {}
            }
        }
    }

    /// Sends T3 Code's restart continuation.
    fn continue_prompt(&mut self) -> Continued {
        let (reply, updates) = self
            .server
            .prompt_v2(&self.session_id, "Continue where you left off.");
        reply.unwrap();
        Continued {
            stop_reason: updates.last().unwrap()["stopReason"].clone(),
            answer: updates
                .iter()
                .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
                .filter_map(|update| update["content"]["text"].as_str())
                .collect(),
            ran_tool: updates
                .iter()
                .any(|update| update["sessionUpdate"] == "tool_call_update"),
        }
    }
}

/// How the continuation prompt went.
#[derive(Debug, PartialEq)]
struct Continued {
    stop_reason: Value,
    answer: String,
    /// The fixture model calls a tool on every turn, so a tool call means
    /// the prompt ran a model turn.
    ran_tool: bool,
}

fn t3_server(workspace: &Workspace) -> AcpServer {
    let mut command = AcpServer::command(
        &workspace.home,
        "read-path",
        &workspace.extra.join("notes.txt"),
    );
    let mut server = AcpServer::from_command(command.arg("--t3code"));
    server
        .request(
            "initialize",
            json!({
                "protocolVersion": 2,
                "info": {"name": "t3-code", "version": "1.0.0"},
                "capabilities": {},
            }),
        )
        .0
        .unwrap();
    server
}

// Covers: a watcher answers, then T3 Code restarts before it saw the answer
// and sends "Continue where you left off." to the watcher thread. The daemon
// must repeat the answer, the parent's result or the parent-ended notice,
// with no model turn: a model turn there wakes a model in the watcher and
// hands its parent a bogus result.
// Owner: daemon T3 wake watcher, over the real stdio server
#[test]
fn restarted_watcher_repeats_its_answer_without_a_model_turn() {
    for parent in [Parent::Signalled, Parent::Ended] {
        let mut watcher = Watcher::start();
        let expected = watcher.parent(parent);
        let answered = watcher.finish_wake("end_turn");
        // T3 Code restarts: the old `rho acp` dies, a new one attaches.
        let restarted = t3_server(&watcher.workspace);
        drop(std::mem::replace(&mut watcher.server, restarted));
        watcher.load();

        assert_eq!(
            (answered, watcher.continue_prompt()),
            (
                expected.to_owned(),
                Continued {
                    stop_reason: json!("end_turn"),
                    answer: expected.to_owned(),
                    ran_tool: false,
                }
            ),
            "{parent:?}"
        );
    }
}

/// How the host ends a watcher.
#[derive(Clone, Copy, Debug)]
enum Ending {
    /// `session/cancel` while it waits.
    Cancel,
    /// `session/close` while it waits, as T3's task_cancel does.
    Close,
    /// `session/delete` once its wait answered.
    DeleteAnswered,
}

// Covers: a cancel or close, as T3 Code's task_cancel sends, ends a waiting
// watcher for good, and a delete drops an answered one's record: a later
// prompt of the session runs as an ordinary model turn instead of reviving
// the wait or repeating its answer, and no record is left behind.
// Owner: daemon T3 wake watcher, over the real stdio server
#[test]
fn ended_watcher_drops_its_wait() {
    // With whether the session's next prompt ran a model turn; a deleted
    // session has none.
    for (ending, expected_model_turn) in [
        (Ending::Cancel, Some(true)),
        (Ending::Close, Some(true)),
        (Ending::DeleteAnswered, None),
    ] {
        let mut watcher = Watcher::start();
        let session_id = watcher.session_id.clone();
        match ending {
            Ending::Cancel => {
                watcher.server.send(&json!({
                    "jsonrpc": "2.0", "method": "session/cancel",
                    "params": {"sessionId": session_id},
                }));
                watcher.finish_wake("cancelled");
            }
            Ending::Close => {
                let reply = watcher.request_ending_wake(
                    "session/close",
                    json!({"sessionId": session_id}),
                    "cancelled",
                );
                assert!(reply.get("error").is_none(), "{reply}");
            }
            Ending::DeleteAnswered => {
                watcher.parent(Parent::Ended);
                watcher.finish_wake("end_turn");
                let reply =
                    watcher.request_beside_wake("session/delete", json!({"sessionId": session_id}));
                assert!(reply.get("error").is_none(), "{reply}");
            }
        }
        let record_kept = watcher.record().exists();
        let model_turn = match ending {
            Ending::Cancel | Ending::Close => {
                watcher.load();
                let continued = watcher.continue_prompt();
                assert_eq!(continued.stop_reason, json!("end_turn"), "{ending:?}");
                Some(continued.ran_tool)
            }
            Ending::DeleteAnswered => None,
        };

        assert_eq!(
            (record_kept, model_turn),
            (false, expected_model_turn),
            "{ending:?}"
        );
    }
}
