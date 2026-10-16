//! `rho acp` driven over stdio the way an editor host drives it. The
//! automation fixture provider stands in for the model, so each prompt
//! deterministically calls one tool and replies with its result.

#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tempfile::TempDir;

/// Failure bound for one reply; the fixture answers without network.
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// One `rho acp` process and the JSON-RPC client end of its stdio.
struct AcpServer {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: u64,
}

impl AcpServer {
    /// Starts `rho acp` in `cwd` with the fixture provider in `mode`;
    /// `path` is the file the fixture's tool call targets.
    fn spawn(home: &Path, cwd: &Path, mode: &str, path: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rho"))
            .current_dir(cwd)
            .env("HOME", home)
            .env("RHO_HOME", home.join(".rho"))
            .env("RHO_AUTOMATION_TEST_MODE", mode)
            .env("RHO_AUTOMATION_TEST_PATH", path)
            .env_remove("RHO_AUTOMATION_TEST_RESPONSE")
            .env_remove("HERDR_ENV")
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_PANE_ID")
            .arg("--config")
            .arg(home.join("config.toml"))
            .arg("acp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn rho acp");
        let stdin = child.stdin.take().expect("acp stdin");
        let stdout = child.stdout.take().expect("acp stdout");
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
            stdin,
            messages,
            next_id: 0,
        }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").expect("write request");
        self.stdin.flush().expect("flush request");
    }

    /// Sends `method` and waits for its reply. Returns the `result` or the
    /// `error` object, plus the `session/update` payloads that came first.
    fn request(&mut self, method: &str, params: Value) -> (Result<Value, Value>, Vec<Value>) {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut updates = Vec::new();
        loop {
            let message = self
                .messages
                .recv_timeout(REPLY_TIMEOUT)
                .unwrap_or_else(|error| panic!("no reply to {method}: {error}"));
            if message["id"] == id && message.get("method").is_none() {
                let reply = match message.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(message["result"].clone()),
                };
                return (reply, updates);
            }
            match message["method"].as_str() {
                Some("session/update") => updates.push(message["params"]["update"].clone()),
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
}

impl Drop for AcpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A home with a plan-mode config, a project, and a directory outside it
/// holding `notes.txt`.
struct Workspace {
    home: TempDir,
    project: PathBuf,
    extra: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            r#"provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"

[behavior]
permission_mode = "plan"

[web_search]
hosted = false
provider = "disabled"
"#,
        )
        .unwrap();
        let root = home.path().canonicalize().unwrap();
        let project = root.join("project");
        let extra = root.join("shared-lib");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(extra.join("notes.txt"), "extra root content").unwrap();
        Self {
            home,
            project,
            extra,
        }
    }

    fn server(&self) -> AcpServer {
        AcpServer::spawn(
            self.home.path(),
            &self.project,
            "read-path",
            &self.extra.join("notes.txt"),
        )
    }
}

/// Whether the fixture's read of `notes.txt` returned the file.
fn read_the_extra_file(reply: &str) -> bool {
    reply.lines().last() == Some("1:extra root content")
}

// Covers: additionalDirectories on session/new and session/load must put
// those roots in the session's workspace scope (plan mode denies reads
// outside it), each request must set the active roots without reviving
// stored ones, and initialize must advertise the capability so hosts such
// as Zed send them at all.
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

    for (label, additional, readable) in [
        ("load without roots", json!([]), false),
        ("load with roots", json!([extra]), true),
    ] {
        let (loaded, _) = server.request(
            "session/load",
            json!({
                "sessionId": session_id,
                "cwd": workspace.project,
                "mcpServers": [],
                "additionalDirectories": additional,
            }),
        );
        loaded.unwrap_or_else(|error| panic!("{label}: {error}"));
        let reply = server.prompt(&session_id, "read the shared notes");
        assert_eq!(read_the_extra_file(&reply), readable, "{label}: {reply}");
    }
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
