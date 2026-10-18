//! Sign-in recovery against a provider that checks its key over real HTTP.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_providers::credentials::{CredentialStore, FileCredentialStore};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::AcpServer;

const FIRST_KEY: &str = "test-first-key";
const SECOND_KEY: &str = "test-second-key";
const KEY_ACCOUNT: &str = "provider:mock:api-key";

/// An OpenAI-compatible host that accepts only the key in `valid`, which the
/// test changes to revoke the one the session holds. Refusals quote the
/// presented key, as some providers echo request details, so a leak into the
/// ACP error is visible.
fn spawn_provider(valid: Arc<Mutex<String>>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let valid = Arc::clone(&valid);
            std::thread::spawn(move || answer(stream, &valid));
        }
    });
    address
}

fn answer(stream: TcpStream, valid: &Mutex<String>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let route = line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let (mut length, mut authorization) = (0, String::new());
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap();
            } else if name.eq_ignore_ascii_case("authorization") {
                authorization = value.trim().to_owned();
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let accepted = format!("Bearer {}", valid.lock().unwrap());
    let (status, content_type, response) = if authorization != accepted {
        (
            "401 Unauthorized",
            "application/json",
            json!({"error": {"message": format!("invalid key: {authorization}")}}).to_string(),
        )
    } else if route.ends_with("/models") {
        (
            "200 OK",
            "application/json",
            json!({"data": [{"id": "mock-model"}]}).to_string(),
        )
    } else {
        let chunk = |delta: Value, finish: Value| json!({"id": "fixture", "object": "chat.completion.chunk", "created": 0, "model": "mock-model", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
        (
            "200 OK",
            "text/event-stream",
            format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                chunk(
                    json!({"role": "assistant", "content": "answered"}),
                    Value::Null
                ),
                chunk(json!({}), json!("stop"))
            ),
        )
    };
    let reply = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
        response.len()
    );
    let _ = reader.get_mut().write_all(reply.as_bytes());
}

// Covers: a missing key and a key the provider revokes mid-session must both
// answer ACP auth_required (not internal_error), for turns and compaction,
// without echoing the key or the provider's diagnostic, and each sign-in must
// apply without reconnecting: startup runs again, and the next prompt reloads
// the refused credentials for turns and compaction alike, because providers
// capture them when built.
// Owner: ACP request error mapping and session provider lifecycle, over real
// stdio and HTTP; no fixture provider stands in for the credential path.
#[test]
fn credential_failures_require_auth_and_each_sign_in_applies_in_place() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let address = spawn_provider(Arc::clone(&valid));
    let home = TempDir::new().unwrap();
    let project = home.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let config = home.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"provider = "mock"
model = "mock-model"
auth = "mock-api-key"

[providers.custom.mock]
base_url = "http://{address}/v1"

[behavior]
permission_mode = "plan"
check_for_updates = false

[web_search]
hosted = false
provider = "disabled"
"#
        ),
    )
    .unwrap();
    let store = FileCredentialStore::with_rho_home(home.path().join(".rho")).unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_rho"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("RHO_HOME", home.path().join(".rho"))
        .env("XDG_CACHE_HOME", home.path().join(".cache"))
        .env("RHO_CREDENTIAL_STORE", "file")
        .current_dir(&project)
        .arg("--config")
        .arg(&config)
        // An explicit provider makes startup discover its models, so the
        // missing key fails startup itself rather than the session build.
        .args(["--provider", "mock", "acp"]);
    let mut server = AcpServer::from_command(&mut command);
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let auth_required = |reply: Result<Value, Value>, what: &str| {
        let error = reply.expect_err(what);
        assert_eq!(
            (error["code"].clone(), error.get("data")),
            (json!(-32000), None),
            "{what}: {error}"
        );
        assert!(!error.to_string().contains(FIRST_KEY), "{error}");
    };
    let new_session = json!({"cwd": project, "mcpServers": []});
    auth_required(
        server.request("session/new", new_session.clone()).0,
        "no key is saved yet",
    );

    // A sign-in saves the key; the same connection starts without reconnecting.
    store.set_secret(KEY_ACCOUNT, FIRST_KEY).unwrap();
    let (session, _) = server.request("session/new", new_session);
    let session_id = session.unwrap()["sessionId"].as_str().unwrap().to_owned();
    let prompt =
        |text: &str| json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]});
    assert_eq!(server.prompt(&session_id, "hi"), "answered");

    *valid.lock().unwrap() = SECOND_KEY.to_owned();
    auth_required(
        server.request("session/prompt", prompt("hi again")).0,
        "the provider revoked the key",
    );
    auth_required(
        server.request("session/prompt", prompt("/compact")).0,
        "compaction is refused too",
    );

    store.set_secret(KEY_ACCOUNT, SECOND_KEY).unwrap();
    assert_eq!(server.prompt(&session_id, "signed in"), "answered");
    // Compaction summarizes with its own handle on the provider.
    server
        .request("session/prompt", prompt("/compact"))
        .0
        .expect("compaction uses the renewed key");
}
