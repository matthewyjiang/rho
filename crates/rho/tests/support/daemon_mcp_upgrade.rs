//! `rho mcp serve --http` upgrading in place through the real binary: once
//! its executable is replaced, it finishes the calls under way and
//! re-executes the replacement as the same process, on the same address; a
//! replacement that is not rho runs once and is never re-executed.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use pretty_assertions::assert_eq;
use reqwest::Method;
use serde_json::{json, Value};

use super::{
    bash, call, http_client, http_request, initialize, listening, next_line, rotate, Sandbox,
    BLOCKED, LISTENING,
};

/// `rho` installed at a path a test replaces, as an upgrade does: at first
/// a symlink to the binary under test.
struct Install {
    dir: tempfile::TempDir,
    rho: PathBuf,
}

impl Install {
    fn new() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let rho = dir.path().join("rho");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_rho"), &rho).unwrap();
        Self { dir, rho }
    }

    /// A file beside the install, for a replacement to mark.
    fn beside(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Replaces the install by a rename, as an installer does, with a shell
    /// script of `body`.
    fn replace(&self, body: &str) {
        let staged = self.beside("rho.new");
        fs::write(&staged, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&staged, &self.rho).unwrap();
    }

    /// Replaces the install with a script that runs the binary under test,
    /// touching `served` first when it serves.
    fn upgrade(&self, served: &Path) {
        self.replace(&format!(
            "if [ \"$1\" = mcp ]; then : > '{}'; fi\nexec '{}' \"$@\"",
            served.display(),
            env!("CARGO_BIN_EXE_rho")
        ));
    }
}

/// The lines a server prints until one starts with `prefix`, that one last.
async fn until_line(stderr: &mut super::Stderr, prefix: &str) -> Vec<String> {
    let mut lines = Vec::new();
    loop {
        let line = next_line(stderr).await;
        let done = line.starts_with(prefix);
        lines.push(line);
        if done {
            return lines;
        }
    }
}

/// How the line a server prints as it begins to upgrade starts.
const UPGRADING: &str = "rho mcp serve: upgrading to ";

/// The result of the tool call `id` that an SSE response `body` carries.
fn sse_result(body: &str, id: u64) -> Value {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .find(|message| message["id"] == id)
        .map(|message| {
            let text = message["result"]["content"][0]["text"].as_str().unwrap();
            serde_json::from_str(text).unwrap()
        })
        .unwrap_or_else(|| panic!("no result for call {id} in {body:?}"))
}

// Covers: a server whose executable `rho update` replaced keeps serving a
// `wait` under way until the turn it waits on ends, then re-executes the
// replacement as the same process on the same address, which serves a new
// host.
// Owner: rho mcp serve upgrade in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_server_finishes_its_calls_then_upgrades_in_place() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let workspace = sandbox.workspace.display().to_string();
    let secret = rotate(
        &sandbox,
        "host",
        &["--scope", "write", "--workspace", &workspace],
    );
    let bearer = format!("Bearer {secret}");
    let install = Install::new();
    let (mut server, mut stderr, url) =
        listening(sandbox.rho_at(&install.rho, "tool-call"), &[]).await;
    let host = http_client(&url, &secret).await;
    let started = call(
        &host,
        "new",
        json!({"workspace": workspace, "prompt": bash(BLOCKED)}),
    )
    .await
    .unwrap();

    // A raw `wait`, whose response headers arrive once the server took it.
    let initialized = initialize(&url, &[("Authorization", &bearer)]).await;
    let mcp_session = initialized.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(initialized);
    let headers = [
        ("Authorization", bearer.as_str()),
        ("Mcp-Session-Id", mcp_session.as_str()),
    ];
    http_request(
        &url,
        Method::POST,
        &headers,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
    )
    .await;
    let waiting = http_request(
        &url,
        Method::POST,
        &headers,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "wait",
                "arguments": {"session": started["session"], "timeout_secs": 50},
            },
        })
        .to_string(),
    )
    .await;

    let served = install.beside("served");
    install.upgrade(&served);
    until_line(&mut stderr, UPGRADING).await;
    fs::write(sandbox.workspace.join("go"), "").unwrap();
    let waited = sse_result(&waiting.text().await.unwrap(), 3);
    let relistening = next_line(&mut stderr).await;
    let after = http_client(&url, &secret).await;
    let listed = call(&after, "list", json!({})).await;

    assert_eq!(
        (
            waited["state"].clone(),
            relistening.strip_prefix(LISTENING),
            served.exists(),
            server.try_wait().unwrap(),
            listed.map(|list| list["sessions"][0]["session"].clone()),
        ),
        (
            json!("completed"),
            Some(url.as_str()),
            true,
            None,
            Ok(started["session"].clone()),
        ),
        "daemon log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a replacement that is not rho runs with `--version` once, however
// many hosts connect afterwards, and the server keeps serving on its binary
// until a real replacement arrives, which it then upgrades to.
// Owner: rho mcp serve upgrade in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replacement_that_is_not_rho_runs_once() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let sandbox = Sandbox::new();
    let workspace = sandbox.workspace.display().to_string();
    let secret = rotate(
        &sandbox,
        "host",
        &["--scope", "read", "--workspace", &workspace],
    );
    let install = Install::new();
    let (_server, mut stderr, url) =
        listening(sandbox.rho_at(&install.rho, "tool-call"), &[]).await;
    let runs = install.beside("runs");

    install.replace(&format!("echo run >> '{}'\nexit 1", runs.display()));
    let refused = next_line(&mut stderr).await;
    let mut listed = Vec::new();
    for _ in 0..5 {
        // A client of its own, so each call comes on a new connection.
        let host = http_client(&url, &secret).await;
        listed.push(call(&host, "list", json!({})).await.is_ok());
    }
    install.upgrade(&install.beside("served"));
    let until_upgrading = until_line(&mut stderr, UPGRADING).await;

    assert_eq!(
        (
            refused.starts_with("rho mcp serve: "),
            listed,
            fs::read_to_string(&runs).unwrap(),
            until_upgrading.len(),
        ),
        (true, vec![true; 5], "run\n".to_owned(), 1),
    );
}
