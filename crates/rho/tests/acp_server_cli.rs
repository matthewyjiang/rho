//! `rho acp` driven over stdio the way an editor host drives it. The
//! automation fixture provider stands in for the model, so each prompt
//! deterministically calls one tool and replies with its result.
//!
//! `rho acp` is a client of the rho daemon, which it starts in the
//! sandbox; the daemon inherits the first client's environment, fixture
//! mode included, and the sandbox stops it when it drops, after the server.
//! The daemon is unix-only (#1665), and so is `rho acp`.
#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_tui_pty::IsolatedHome;
use serde_json::{json, Value};

#[path = "support/acp_advertisements_tests.rs"]
mod advertisements;

#[path = "support/acp_agent_model_override.rs"]
mod agent_model_override;

#[path = "support/acp_auth_required.rs"]
mod auth_required;

#[path = "support/key_checking_provider.rs"]
mod key_checking_provider;

#[path = "support/acp_compaction_cards_tests.rs"]
mod compaction_cards;

#[path = "support/acp_fast_mode_tests.rs"]
mod fast_mode;

#[path = "support/acp_file_diffs_tests.rs"]
mod file_diffs;

#[path = "support/acp_login.rs"]
mod login;

#[path = "support/acp_protocol_v2_tests.rs"]
mod protocol_v2;

#[path = "support/acp_questions_tests.rs"]
mod questions;

#[path = "support/acp_session_lifecycle_tests.rs"]
mod session_lifecycle;

#[path = "support/acp_session_titles_tests.rs"]
mod session_titles;

#[path = "support/acp_stop_reasons_tests.rs"]
mod stop_reasons;

#[path = "support/acp_t3_wake_tests.rs"]
mod t3_wake;

#[path = "support/acp_update_metadata_tests.rs"]
mod update_metadata;

#[path = "support/acp_usage_cost_tests.rs"]
mod usage_cost;

/// Failure bound for one reply; the fixture answers without network.
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// One `rho acp` process and the JSON-RPC client end of its stdio.
struct AcpServer {
    child: Child,
    /// `None` once [`Self::quit`] closed it.
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    /// Everything the agent wrote to stderr, shown when it stops answering.
    stderr: Arc<Mutex<String>>,
    next_id: u64,
}

impl AcpServer {
    /// Starts `rho acp` in `home`'s sandbox and workspace, with the fixture
    /// provider in `mode`; `path` is the file the fixture's tool call
    /// targets.
    fn spawn(home: &IsolatedHome, mode: &str, path: &Path) -> Self {
        Self::from_command(&mut Self::command(home, mode, path))
    }

    /// The command [`Self::spawn`] runs, for tests that add to it.
    fn command(home: &IsolatedHome, mode: &str, path: &Path) -> Command {
        let mut command = home.command(env!("CARGO_BIN_EXE_rho"));
        command
            .env("RHO_AUTOMATION_TEST_MODE", mode)
            .env("RHO_AUTOMATION_TEST_PATH", path)
            .arg("acp");
        command
    }

    fn from_command(command: &mut Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rho acp");
        let stdin = child.stdin.take().expect("acp stdin");
        let stdout = child.stdout.take().expect("acp stdout");
        let stderr = Arc::new(Mutex::new(String::new()));
        let mut agent_stderr = child.stderr.take().expect("acp stderr");
        std::thread::spawn({
            let stderr = Arc::clone(&stderr);
            move || {
                let mut chunk = [0; 4096];
                while let Ok(read @ 1..) = agent_stderr.read(&mut chunk) {
                    stderr
                        .lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&chunk[..read]));
                }
            }
        });
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                let message = serde_json::from_str(&line).expect("stdout is JSON-RPC only");
                if sender.send(message).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            messages,
            stderr,
            next_id: 0,
        }
    }

    /// The next message from the agent; panics naming `what` with the
    /// agent's stderr if none arrives.
    fn recv(&self, what: &str) -> Value {
        self.messages
            .recv_timeout(REPLY_TIMEOUT)
            .unwrap_or_else(|error| {
                panic!(
                    "no {what}: {error}\nagent stderr:\n{}",
                    self.stderr.lock().unwrap()
                )
            })
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("agent stdin is open");
        writeln!(stdin, "{message}").expect("write request");
        stdin.flush().expect("flush request");
    }

    /// Closes stdin, as a host that quits does, and waits for the agent to
    /// shut down.
    fn quit(mut self) {
        drop(self.stdin.take());
        self.child.wait().expect("wait for rho acp");
    }

    /// Sends `method` and waits for its reply. Returns the `result` or the
    /// `error` object, plus the `session/update` payloads that came first.
    fn request(&mut self, method: &str, params: Value) -> (Result<Value, Value>, Vec<Value>) {
        let (reply, notifications) = self.request_notifications(method, params);
        let updates = notifications
            .into_iter()
            .map(|notification| notification["update"].clone())
            .collect();
        (reply, updates)
    }

    /// Like `request`, retaining the session id and replay metadata.
    fn request_notifications(
        &mut self,
        method: &str,
        params: Value,
    ) -> (Result<Value, Value>, Vec<Value>) {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut updates = Vec::new();
        loop {
            let message = self.recv(&format!("reply to {method}"));
            if message["id"] == id && message.get("method").is_none() {
                let reply = match message.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(message["result"].clone()),
                };
                return (reply, updates);
            }
            match message["method"].as_str() {
                Some("session/update") => updates.push(message["params"].clone()),
                Some(other) => panic!("unexpected agent message {other}: {message}"),
                None => panic!("reply to an unknown request: {message}"),
            }
        }
    }

    /// Sends one text prompt and returns the agent's reply text.
    fn prompt(&mut self, session_id: &str, text: &str) -> String {
        let (reply, updates) = self.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
        );
        reply.unwrap_or_else(|error| panic!("prompt failed: {error}"));
        updates
            .iter()
            .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
            .filter_map(|update| update["content"]["text"].as_str())
            .collect()
    }

    /// The next `session/update` the agent sends without a pending request,
    /// such as a title that lands after the turn it names.
    fn next_notification(&mut self) -> Value {
        let message = self.recv("session/update");
        assert_eq!(message["method"], "session/update", "{message}");
        message["params"].clone()
    }
}

impl Drop for AcpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A sandbox with a plan-mode config, its workspace as the project, and a
/// directory outside the project holding `notes.txt`.
struct Workspace {
    home: IsolatedHome,
    /// The sandbox's workspace, canonicalized as rho reports it.
    project: PathBuf,
    extra: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let home = IsolatedHome::new().unwrap();
        std::fs::write(
            &home.config_path,
            r#"[model]
provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"

[behavior]
permission_mode = "plan"

[web_search]
mode = "off"
"#,
        )
        .unwrap();
        let project = home.workspace.canonicalize().unwrap();
        let extra = home.home.canonicalize().unwrap().join("shared-lib");
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(extra.join("notes.txt"), "extra root content").unwrap();
        Self {
            home,
            project,
            extra,
        }
    }

    fn server(&self) -> AcpServer {
        AcpServer::spawn(&self.home, "read-path", &self.extra.join("notes.txt"))
    }
}

/// Stops `home`'s daemon, so the next `rho acp` starts a fresh one that
/// reopens sessions from their files, as after a restart.
fn stop_daemon(home: &IsolatedHome) {
    let status = home
        .command(env!("CARGO_BIN_EXE_rho"))
        .args(["daemon", "stop"])
        .stdin(Stdio::null())
        .status()
        .expect("run rho daemon stop");
    assert!(status.success(), "rho daemon stop: {status}");
}

/// Whether the fixture's read of `notes.txt` returned the file.
fn read_the_extra_file(reply: &str) -> bool {
    reply.lines().last() == Some("1:extra root content")
}

// Covers: additionalDirectories on session/new must put those roots in the
// session's workspace scope (plan mode denies reads outside it), the
// session keeps them when a later load names none, even once a daemon
// restart reopens it from its file (#1661), and initialize must advertise
// the capability so hosts such as Zed send them at all.
// Owner: ACP session lifecycle, over the real stdio server
#[test]
fn additional_directories_scope_new_and_loaded_sessions() {
    let workspace = Workspace::new();
    let mut server = workspace.server();
    let (initialized, _) = server.request("initialize", json!({"protocolVersion": 1}));
    assert_eq!(
        initialized.unwrap()["agentCapabilities"]["sessionCapabilities"]["additionalDirectories"],
        json!({})
    );

    let extra = workspace.extra.to_str().unwrap();
    let (created, _) = server.request(
        "session/new",
        json!({
            "cwd": workspace.project,
            "mcpServers": [],
            "additionalDirectories": [extra],
        }),
    );
    let session_id = created.unwrap()["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let reply = server.prompt(&session_id, "read the shared notes");
    assert!(read_the_extra_file(&reply), "{reply}");

    drop(server);
    stop_daemon(&workspace.home);
    let mut server = workspace.server();
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let (loaded, _) = server.request(
        "session/load",
        json!({
            "sessionId": session_id,
            "cwd": workspace.project,
            "mcpServers": [],
            "additionalDirectories": [],
        }),
    );
    loaded.unwrap();
    let reply = server.prompt(&session_id, "read the shared notes");
    assert!(read_the_extra_file(&reply), "{reply}");
}

// Covers: an additionalDirectories entry the session cannot grant must fail
// session/new with invalid params, never start a session with fewer roots.
// Owner: ACP session lifecycle, over the real stdio server
#[test]
fn additional_directories_reject_entries_they_cannot_grant() {
    let workspace = Workspace::new();
    let mut server = workspace.server();
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let missing = workspace.extra.join("missing");
    let file = workspace.extra.join("notes.txt");
    for (label, entry) in [
        ("relative", json!("shared-lib")),
        ("empty", json!("")),
        ("missing", json!(missing)),
        ("file", json!(file)),
    ] {
        let (created, _) = server.request(
            "session/new",
            json!({
                "cwd": workspace.project,
                "mcpServers": [],
                "additionalDirectories": [workspace.extra, entry],
            }),
        );
        let error = created.expect_err(label);
        assert_eq!(error["code"], -32602, "{label}: {error}");
    }
}

// Covers: loading saved history after a restart of `rho acp` and its daemon
// must restore paired tool cards, arguments, result status and output before
// session/load replies, even when successive provider turns reuse the same
// tool-call ID.
// Owner: ACP session replay, over the real stdio server
#[test]
fn load_replays_tool_calls_and_results() {
    for (additional, status) in [(true, "completed"), (false, "failed")] {
        let workspace = Workspace::new();
        let mut server = workspace.server();
        server
            .request("initialize", json!({"protocolVersion": 1}))
            .0
            .unwrap();
        let roots = if additional {
            json!([workspace.extra])
        } else {
            json!([])
        };
        let created = server
            .request(
                "session/new",
                json!({"cwd": workspace.project, "mcpServers": [], "additionalDirectories": roots}),
            )
            .0
            .unwrap();
        let session_id = created["sessionId"].as_str().unwrap();
        let mut expected = Vec::new();
        let mut live_ids = Vec::new();
        for text in ["first output", "second output"] {
            std::fs::write(workspace.extra.join("notes.txt"), text).unwrap();
            // Each daemon builds a new fixture provider, whose first response
            // reuses the tool-call ID the previous daemon's did.
            let (reply, live) = server.request(
                "session/prompt",
                json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "read the shared notes"}]}),
            );
            reply.unwrap();
            let call = live
                .iter()
                .find(|u| u["sessionUpdate"] == "tool_call")
                .unwrap();
            let result = live
                .iter()
                .find(|u| u["sessionUpdate"] == "tool_call_update" && u["status"] == status)
                .expect("live terminal result");
            assert_ne!(result["content"], json!([]));
            live_ids.push(call["toolCallId"].clone());
            expected.push(json!({"name": call["name"], "rawInput": call["rawInput"],
                "status": status, "content": result["content"]}));
            server
                .request("session/close", json!({"sessionId": session_id}))
                .0
                .unwrap();
            drop(server);
            // A fresh daemon reopens the session from its file, with a
            // fresh fixture provider.
            stop_daemon(&workspace.home);
            server = workspace.server();
            server
                .request("initialize", json!({"protocolVersion": 1}))
                .0
                .unwrap();
            let (loaded, notifications) = server.request_notifications(
                "session/load",
                json!({"sessionId": session_id, "cwd": workspace.project, "mcpServers": [], "additionalDirectories": roots}),
            );
            loaded.unwrap();
            let tools = notifications
                .iter()
                .filter(|n| {
                    matches!(
                        n["update"]["sessionUpdate"].as_str(),
                        Some("tool_call" | "tool_call_update")
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(tools.len(), expected.len() * 2, "{notifications:#?}");
            let mut ids = std::collections::BTreeSet::new();
            let restored = tools
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    let call = &pair[0]["update"];
                    let result = &pair[1]["update"];
                    assert!(
                        ids.insert(call["toolCallId"].as_str().unwrap()),
                        "duplicate replay ID: {tools:#?}"
                    );
                    assert_eq!(result["toolCallId"], call["toolCallId"]);
                    for notification in pair {
                        assert_eq!(
                            (&notification["sessionId"], &notification["_meta"]),
                            (&json!(session_id), &json!({"isReplay": true}))
                        );
                    }
                    json!({"name": call["name"], "rawInput": call["rawInput"],
                    "status": result["status"], "content": result["content"]})
                })
                .collect::<Vec<_>>();
            assert_eq!(restored, expected);
        }
        // The session already holds the reused ID, so the second call
        // gets its own wire ID, live as in replay.
        assert_ne!(live_ids[0], live_ids[1]);
    }
}
