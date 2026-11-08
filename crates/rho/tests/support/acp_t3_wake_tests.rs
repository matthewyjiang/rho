//! T3 Code wake watchers (`rho acp --t3code`) over the real ACP v2 stdio
//! server and the daemon behind it.
//!
//! The test plays the watcher's parent: it holds the parent's lock on the
//! signal, writes the signal, and reads what the watcher session answers.

use std::{fs::File, path::PathBuf};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace};

const TOKEN: &str = "0f4e2c3a-9b1d-4e5f-8a7b-6c5d4e3f2a1b";

/// The parent's result, which the watcher answers with.
const RESULT: &str = "cargo test exited 0";

/// A `rho acp --t3code` server with one watcher session waiting on
/// [`TOKEN`], whose parent lock this holds.
struct Watcher {
    /// Before `workspace`, so the server is killed before the sandbox
    /// tears down (see [`rho_tui_pty::IsolatedHome`]).
    server: AcpServer,
    workspace: Workspace,
    session_id: String,
    /// Held so the watcher waits for the signal.
    _parent_lock: File,
}

impl Watcher {
    /// Opens the watcher session and sends its `/rho-wake` prompt, returning
    /// once the daemon accepted it.
    fn start() -> Self {
        let workspace = Workspace::new();
        let signals = workspace.home.rho_home().join("t3-wakes");
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
            _parent_lock: parent_lock,
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
        self.workspace.home.rho_home().join("t3-wakes")
    }

    /// The watcher session's record of its wait.
    fn record(&self) -> PathBuf {
        self.signals().join(format!("{}.watcher", self.session_id))
    }

    /// Plays the parent whose work finished: writes [`RESULT`] whole, as
    /// the daemon does, since a polling watcher takes a partial file.
    fn signal(&self) {
        let staging = self.signals().join(format!("{TOKEN}.tmp"));
        std::fs::write(&staging, RESULT).unwrap();
        std::fs::rename(staging, self.signals().join(TOKEN)).unwrap();
    }

    /// Restarts T3 Code as `restart` says: the old `rho acp` goes, a new one
    /// attaches the watcher session.
    fn restart(&mut self, restart: Restart) {
        let restarted = t3_server(&self.workspace);
        let old = std::mem::replace(&mut self.server, restarted);
        match restart {
            Restart::Quit => old.quit(),
            Restart::Killed => drop(old),
        }
        self.load();
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
// must repeat the answer with no model turn: a model turn there wakes a
// model in the watcher and hands its parent a bogus result.
// Owner: daemon T3 wake watcher, over the real stdio server
#[test]
fn restarted_watcher_repeats_its_answer_without_a_model_turn() {
    let mut watcher = Watcher::start();
    watcher.signal();
    let answered = watcher.finish_wake("end_turn");
    watcher.restart(Restart::Killed);

    assert_eq!(
        (answered, watcher.continue_prompt()),
        (
            RESULT.to_owned(),
            Continued {
                stop_reason: json!("end_turn"),
                answer: RESULT.to_owned(),
                ran_tool: false,
            }
        )
    );
}

/// How T3 Code restarts while a watcher waits.
#[derive(Clone, Copy, Debug)]
enum Restart {
    /// It quits: `rho acp` detaches, which cancels the waiting prompt.
    Quit,
    /// `rho acp` is killed: the wait goes on in the daemon, so the next
    /// prompt finds the session busy with it.
    Killed,
}

// Covers: T3 Code restarts while a watcher waits, then sends "Continue
// where you left off." to the watcher thread. Whether the restart
// cancelled the wait or left it running, the prompt must resume the wait
// and answer with the parent's result, not run a model turn or be refused
// as busy: either wakes the parent early with no result.
// Owner: daemon T3 wake watcher, over the real stdio server
#[test]
fn watcher_cut_off_mid_wait_resumes_it_on_the_next_prompt() {
    for restart in [Restart::Quit, Restart::Killed] {
        let mut watcher = Watcher::start();
        watcher.restart(restart);
        let session_id = watcher.session_id.clone();
        let accepted = watcher.request_beside_wake(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "Continue where you left off."}],
            }),
        );
        let refused = accepted.get("error").cloned();
        // Only once the resumed wait began, so the old one cannot take it.
        watcher.signal();
        let answer = watcher.finish_wake("end_turn");

        assert_eq!((refused, answer), (None, RESULT.to_owned()), "{restart:?}");
    }
}

// Covers: deleting an answered watcher's session drops its record, so the
// wake directory does not keep a file per watcher T3 deleted.
// Owner: daemon T3 wake watcher (session delete), over the real stdio server
#[test]
fn deleted_watcher_drops_its_record() {
    let mut watcher = Watcher::start();
    watcher.signal();
    watcher.finish_wake("end_turn");
    let recorded = watcher.record().exists();
    let session_id = watcher.session_id.clone();

    let reply = watcher.request_beside_wake("session/delete", json!({"sessionId": session_id}));

    assert_eq!(
        (recorded, reply.get("error"), watcher.record().exists()),
        (true, None, false)
    );
}
