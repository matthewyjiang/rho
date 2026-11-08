//! An OpenAI-compatible provider over real HTTP that checks the key every
//! request presents, for tests of signing in and out: ACP's
//! (`acp_auth_required.rs`) and the daemon's (`daemon_auth.rs`).

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};

/// An OpenAI-compatible host that accepts only the key in `valid`, which the
/// test changes to revoke the one the session holds. Refusals quote the
/// presented key, as some providers echo request details, so a leak into a
/// client's error is visible.
pub(crate) fn spawn_provider(valid: Arc<Mutex<String>>) -> SocketAddr {
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
