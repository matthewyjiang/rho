//! Provider finish reasons reported as ACP prompt stop reasons.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::AcpServer;

/// An OpenAI-compatible host that streams `partial` and then finishes with the
/// `finish_reason` the prompt names after `finish=`.
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
    let mut length = 0;
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap();
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let body = String::from_utf8(body).unwrap();
    let (content_type, response) = if route.ends_with("/models") {
        (
            "application/json",
            json!({"data": [{"id": "mock-model"}]}).to_string(),
        )
    } else {
        // History repeats earlier prompts; the latest marker is this prompt's.
        let finish = ["length", "content_filter", "stop"]
            .into_iter()
            .filter_map(|reason| Some((body.rfind(&format!("finish={reason}"))?, reason)))
            .max()
            .map_or("stop", |(_, reason)| reason);
        let chunk = |delta: Value, finish: Value| json!({"id": "fixture", "object": "chat.completion.chunk", "created": 0, "model": "mock-model", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
        (
            "text/event-stream",
            format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                chunk(
                    json!({"role": "assistant", "content": "partial"}),
                    Value::Null
                ),
                chunk(json!({}), json!(finish))
            ),
        )
    };
    let reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
        response.len()
    );
    let _ = reader.get_mut().write_all(reply.as_bytes());
}

// Covers: a turn the provider truncated or content-filtered must end the ACP
// prompt with `max_tokens` / `refusal` instead of `end_turn`, after streaming
// the partial answer, and the session must keep taking prompts.
// Owner: provider finish-reason parsing through the SDK run outcome to the
// ACP prompt response, over real stdio and HTTP.
#[test]
fn provider_finish_reasons_reach_the_prompt_stop_reason() {
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
        .args(["--provider", "mock", "acp"]);
    // Winsock cannot initialize without SystemRoot.
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut server = AcpServer::from_command(&mut command);
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let session = server
        .request("session/new", json!({"cwd": project, "mcpServers": []}))
        .0
        .unwrap();
    let session_id = session["sessionId"].as_str().unwrap().to_owned();

    for (finish, stop_reason) in [
        ("length", "max_tokens"),
        ("content_filter", "refusal"),
        ("stop", "end_turn"),
    ] {
        let (reply, updates) = server.request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": format!("finish={finish}")}],
            }),
        );
        let streamed: String = updates
            .iter()
            .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
            .filter_map(|update| update["content"]["text"].as_str())
            .collect();

        assert_eq!(
            (reply.unwrap()["stopReason"].clone(), streamed),
            (json!(stop_reason), "partial".to_owned()),
            "finish_reason {finish}"
        );
    }
}
