//! A provider that refuses the session's key mid-session, over real HTTP.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
};

use pretty_assertions::assert_eq;
use rho_providers::credentials::{CredentialStore, FileCredentialStore};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::AcpServer;

const STALE_KEY: &str = "test-stale-key";
const FRESH_KEY: &str = "test-fresh-key";
const KEY_ACCOUNT: &str = "provider:mock:api-key";

/// An OpenAI-compatible host that lists one model and answers chat only for
/// [`FRESH_KEY`]. Refusals quote the presented key, as some providers echo
/// request details, so a leak into the ACP error is visible.
fn spawn_provider() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            std::thread::spawn(move || answer(stream));
        }
    });
    address
}

fn answer(stream: TcpStream) {
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
    let (status, content_type, response) = if route.ends_with("/models") {
        (
            "200 OK",
            "application/json",
            json!({"data": [{"id": "mock-model"}]}).to_string(),
        )
    } else if authorization != format!("Bearer {FRESH_KEY}") {
        (
            "401 Unauthorized",
            "application/json",
            json!({"error": {"message": format!("invalid key: {authorization}")}}).to_string(),
        )
    } else {
        let chunk = |delta: Value, finish: Value| json!({"id": "fixture", "object": "chat.completion.chunk", "created": 0, "model": "mock-model", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
        (
            "200 OK",
            "text/event-stream",
            format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                chunk(
                    json!({"role": "assistant", "content": "signed in again"}),
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

// Covers: a key the provider refuses mid-session must answer the prompt with
// ACP auth_required (not internal_error), without echoing the key or the
// provider's diagnostic, and a new login must apply to the next prompt in
// the same session, because providers capture credentials when built.
// Owner: ACP prompt error mapping and session provider lifecycle, over real
// stdio and HTTP; no fixture provider stands in for the credential path.
#[test]
fn refused_credentials_require_auth_and_a_new_login_resumes_the_session() {
    let address = spawn_provider();
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
    store.set_secret(KEY_ACCOUNT, STALE_KEY).unwrap();
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
        .arg("acp");
    let mut server = AcpServer::from_command(&mut command);
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let (session, _) = server.request("session/new", json!({"cwd": project, "mcpServers": []}));
    let session_id = session.unwrap()["sessionId"].as_str().unwrap().to_owned();
    let prompt = json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "hi"}]});

    let (refused, _) = server.request("session/prompt", prompt.clone());
    let refused = refused.expect_err("the provider refuses the stale key");
    assert_eq!(
        (refused["code"].clone(), refused.get("data")),
        (json!(-32000), None)
    );
    assert!(!refused.to_string().contains(STALE_KEY), "{refused}");

    store.set_secret(KEY_ACCOUNT, FRESH_KEY).unwrap();
    assert_eq!(server.prompt(&session_id, "hi again"), "signed in again");
}
