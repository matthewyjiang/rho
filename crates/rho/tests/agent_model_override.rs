//! Real CLI -> agent tool -> child provider requests, with loopback-only hosts.

use std::{
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

// Receipt: this loopback scenario completed in 0.19s locally. A minute is a
// generous deadlock tripwire for slow CI, not synchronization between requests.
const CLI_EXIT_TIMEOUT: Duration = Duration::from_secs(60);

// Covers: selections reach the actual delegated wire request (including slash
// IDs), while omission stays on the parent and reviewers cannot gain write tools.
// Owner: native delegation integration, not catalog policy or TUI rendering.
#[tokio::test]
async fn native_agent_model_override_reaches_child_provider() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let captured = captured.clone();
            tokio::spawn(async move {
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                let route = line.split_whitespace().nth(1).unwrap().to_owned();
                let mut length = 0;
                loop {
                    line.clear();
                    if stream.read_line(&mut line).await.unwrap() == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                        assert!(
                            !name.eq_ignore_ascii_case("authorization"),
                            "keyless fixture must not inherit parent credentials"
                        );
                    }
                }
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                let (content_type, response) = if route.ends_with("/models") {
                    ("application/json", json!({"data": [{"id": if route.starts_with("/child/") { "vendor/review" } else { "parent-model" }}]}).to_string())
                } else {
                    let request: Value = serde_json::from_slice(&body).unwrap();
                    let tools = request["tools"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    let is_root = tools.iter().any(|tool| tool["function"]["name"] == "agent");
                    let already_delegated = request["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|message| message["role"] == "tool");
                    let delta = if is_root && !already_delegated {
                        let calls = [None, Some("child/vendor/review"), Some("@review"), Some("xai/grok-4.7"), Some("")].into_iter().enumerate().map(|(index, model)| {
                            let mut arguments = json!({"agent_id": "reviewer", "prompt": "Review this fixture."});
                            if let Some(model) = model { arguments["model"] = json!(model); }
                            json!({"index": index, "id": format!("review-{index}"), "type": "function", "function": {"name": "agent", "arguments": arguments.to_string()}})
                        }).collect::<Vec<_>>();
                        json!({"role": "assistant", "tool_calls": calls})
                    } else {
                        json!({"role": "assistant", "content": "review complete"})
                    };
                    let finish = if delta.get("tool_calls").is_some() {
                        "tool_calls"
                    } else {
                        "stop"
                    };
                    let chunk = |delta: Value, finish: Value| {
                        json!({"id": "fixture", "object": "chat.completion.chunk", "created": 0, "model": request["model"], "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}).to_string()
                    };
                    let response = format!(
                        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                        chunk(delta, Value::Null),
                        chunk(json!({}), json!(finish))
                    );
                    captured.lock().unwrap().push((route.clone(), request));
                    ("text/event-stream", response)
                };
                let reply = format!("HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}", response.len());
                stream.write_all(reply.as_bytes()).await.unwrap();
            });
        }
    });
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"
[model]
provider = "parent"
model = "parent-model"
auth = "none"
[model.aliases]
review = "child/vendor/review"
[providers.custom.parent]
base_url = "http://{address}/parent/v1"
[providers.custom.child]
base_url = "http://{address}/child/v1"
[behavior]
permission_mode = "plan"
check_for_updates = false
credential_store = "file"
"#
        ),
    )
    .unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rho"));
    command
        .current_dir(root.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", root.path())
        .env("RHO_HOME", root.path().join(".rho"))
        .env("XDG_CACHE_HOME", root.path().join(".cache"))
        .arg("--config")
        .arg(&config)
        .args([
            "run",
            "--output",
            "jsonl",
            "Review using the delegated agents.",
        ])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(CLI_EXIT_TIMEOUT, command.output())
        .await
        .expect("CLI exit timeout exceeded (60s)")
        .unwrap();
    server.abort();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let events = String::from_utf8(output.stdout).unwrap();
    let mut receipts = events
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["type"] == "tool.finished")
        .map(|event| {
            (
                event["call_id"].as_str().unwrap().to_owned(),
                event["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    receipts.sort_unstable();
    assert_eq!(
        receipts,
        [
            ("review-0".into(), "success".into()),
            ("review-1".into(), "success".into()),
            ("review-2".into(), "success".into()),
            ("review-3".into(), "failure".into()),
            ("review-4".into(), "failure".into()),
        ]
    );
    let requests = requests.lock().unwrap();
    let mut reviewers = requests
        .iter()
        .filter_map(|(route, request)| {
            let mut tools = request["tools"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap())
                .collect::<Vec<_>>();
            if !tools.contains(&"read_file") || tools.contains(&"agent") {
                return None;
            }
            tools.sort_unstable();
            assert_eq!(
                tools,
                [
                    "codemode",
                    "glob",
                    "grep",
                    "list_dir",
                    "message_parent",
                    "read_file",
                    "request_parent_action",
                    "skill",
                    "tool_search"
                ]
            );
            Some((route.as_str(), request["model"].as_str().unwrap()))
        })
        .collect::<Vec<_>>();
    reviewers.sort_unstable();
    assert_eq!(
        reviewers,
        [
            ("/child/v1/chat/completions", "vendor/review"),
            ("/child/v1/chat/completions", "vendor/review"),
            ("/parent/v1/chat/completions", "parent-model"),
        ]
    );
}
