//! An OpenAI-compatible Chat Completions host over real HTTP, for tests of
//! a custom provider the config adds or edits while a daemon session is
//! open (`daemon_config.rs`). It answers each chat request with what the
//! test's script returns for it, and records the model each one named.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};

/// What the host answers one chat request with.
pub(crate) enum Reply {
    Text(&'static str),
    /// One call of the tool `name` with `arguments`.
    ToolCall {
        name: &'static str,
        arguments: Value,
    },
}

/// A running host.
pub(crate) struct ScriptedHost {
    address: SocketAddr,
    /// The `model` of each chat request, in the order they arrived.
    models: Arc<Mutex<Vec<String>>>,
}

impl ScriptedHost {
    /// A host that lists `models` and answers each chat request with what
    /// `script` returns for its JSON body. The script runs on the request's
    /// own thread, so it may block to hold the answer back.
    pub(crate) fn spawn(
        models: &'static [&'static str],
        script: impl Fn(&Value) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requested = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(script);
        let recorder = Arc::clone(&requested);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (script, recorder) = (Arc::clone(&script), Arc::clone(&recorder));
                std::thread::spawn(move || answer(stream, models, &*script, &recorder));
            }
        });
        Self {
            address,
            models: requested,
        }
    }

    /// `[providers.custom.<name>]` pointing at this host.
    pub(crate) fn provider_table(&self, name: &str) -> String {
        format!(
            "[providers.custom.{name}]\nbase_url = \"http://{}/v1\"\n",
            self.address
        )
    }

    /// The models the chat requests so far named, in order.
    pub(crate) fn models(&self) -> Vec<String> {
        self.models.lock().unwrap().clone()
    }
}

fn answer(
    stream: TcpStream,
    models: &[&str],
    script: &(dyn Fn(&Value) -> Reply + Send + Sync),
    recorder: &Mutex<Vec<String>>,
) {
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
    let request = serde_json::from_slice::<Value>(&body);
    let (status, content_type, response) = if route.ends_with("/models") {
        let data = models
            .iter()
            .map(|id| json!({"id": id}))
            .collect::<Vec<_>>();
        (
            "200 OK",
            "application/json",
            json!({"data": data}).to_string(),
        )
    } else if let Ok(request) = request {
        let model = request["model"].as_str().unwrap_or_default().to_owned();
        recorder.lock().unwrap().push(model.clone());
        let chunk = |delta: Value, finish: Value| json!({"id": "scripted", "object": "chat.completion.chunk", "created": 0, "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
        let (delta, finish) = match script(&request) {
            Reply::Text(text) => (json!({"role": "assistant", "content": text}), "stop"),
            Reply::ToolCall { name, arguments } => (
                json!({"role": "assistant", "tool_calls": [{"index": 0, "id": "scripted-call", "type": "function", "function": {"name": name, "arguments": arguments.to_string()}}]}),
                "tool_calls",
            ),
        };
        (
            "200 OK",
            "text/event-stream",
            format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                chunk(delta, Value::Null),
                chunk(json!({}), json!(finish))
            ),
        )
    } else {
        // Only chat requests have a body to answer.
        ("404 Not Found", "text/plain", String::new())
    };
    let reply = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
        response.len()
    );
    let _ = reader.get_mut().write_all(reply.as_bytes());
}
