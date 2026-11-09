//! `rho mcp serve` through the real binary: an MCP host drives a daemon
//! session in another workspace over stdio while a second daemon client
//! follows it, no tool can answer the session's approvals, the core's caps
//! refuse with their budgets, and the HTTP transport refuses requests its
//! tokens and origin rules do not allow.

use std::{path::PathBuf, process::Stdio};

use pretty_assertions::assert_eq;
use rho_protocol::{methods::JobList, JobListParams, JobState, SessionEvent, SessionId};
use rmcp::{
    model::CallToolRequestParams,
    service::RunningService,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
        TokioChildProcess,
    },
    RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};

use super::sessions::{Follower, Sandbox, DEADLINE};

type McpClient = RunningService<RoleClient, ()>;

/// A directory beside the sandbox's workspace for sessions to work in, so
/// the server, started in the workspace, drives a session elsewhere.
fn elsewhere(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.isolated.home.join("elsewhere");
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// `rho mcp serve` over stdio, with the fixture in `mode`, `args`, and
/// `env`; the daemon it reaches starts on demand.
async fn stdio_server(
    sandbox: &Sandbox,
    mode: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> McpClient {
    let mut command = tokio::process::Command::from(sandbox.rho(mode));
    command
        .args(["mcp", "serve"])
        .args(args)
        .envs(env.iter().copied());
    let transport = TokioChildProcess::builder(command)
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
        .0;
    ().serve(transport).await.expect("the server initializes")
}

/// Calls `tool` with `arguments`: its JSON result, or the error the model
/// would read.
async fn call(client: &McpClient, tool: &str, arguments: Value) -> Result<Value, String> {
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object");
    };
    let result = tokio::time::timeout(
        DEADLINE,
        client.call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments)),
    )
    .await
    .expect("the tool answers")
    .map_err(|error| error.to_string())?;
    let text = match result.content.first().and_then(|content| content.as_text()) {
        Some(text) => text.text.clone(),
        None => panic!("a text result, not {:?}", result.content),
    };
    if result.is_error == Some(true) {
        Err(text)
    } else {
        Ok(serde_json::from_str(&text).unwrap())
    }
}

/// A `tool-call` fixture prompt that runs `command` in bash.
fn bash(command: &str) -> String {
    json!({"id": "bash", "name": "bash", "arguments": {"command": command}}).to_string()
}

/// A bash command that blocks until `go` exists in its workspace, then
/// prints the depth its session runs at.
const BLOCKED: &str = "while [ ! -e go ]; do sleep 0.02; done; echo \"depth $RHO_SESSION_DEPTH\"";

// Covers: an MCP host starts a session in another workspace, waits past a
// timeout and again from the cursor until the turn ends, reads the final
// answer, which a second daemon client following the session sees too, and
// prompts the session's next turn; the session runs one level deeper than
// its server (#1672).
// Owner: rho mcp serve (stdio) over the session-control core.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_drives_a_session_in_another_workspace_over_mcp() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let workspace = elsewhere(&sandbox);
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;

    let started = call(
        &host,
        "new",
        json!({"workspace": workspace, "prompt": bash(BLOCKED)}),
    )
    .await
    .unwrap();
    let session = SessionId::new(started["session"].as_str().unwrap());
    let first = call(
        &host,
        "wait",
        json!({"session": session, "timeout_secs": 0}),
    )
    .await
    .unwrap();
    let (mut observer, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    std::fs::write(workspace.join("go"), "").unwrap();
    let second = call(
        &host,
        "wait",
        json!({"session": session, "cursor": first["cursor"], "timeout_secs": 50}),
    )
    .await
    .unwrap();
    let read = call(&host, "read", json!({"session": session}))
        .await
        .unwrap();
    let listed = call(&host, "list", json!({"workspace": workspace}))
        .await
        .unwrap();
    observer
        .read_until(|event| matches!(event, SessionEvent::TurnEnded { .. }))
        .await;
    let observed = observer
        .events
        .iter()
        .rev()
        .find_map(|(_, event)| match event {
            SessionEvent::TurnEnded { outcome } => Some(serde_json::to_value(outcome).unwrap()),
            _ => None,
        });
    // The session takes the host's next turn.
    let prompted = call(
        &host,
        "prompt",
        json!({"session": session, "text": bash("echo again")}),
    )
    .await;
    let again = call(
        &host,
        "wait",
        json!({"session": session, "cursor": second["cursor"], "timeout_secs": 50}),
    )
    .await
    .unwrap();

    let answer = second["outcome"]["text"].as_str().unwrap_or_default();
    assert_eq!(
        (
            started["workspace"].clone(),
            first["done"].clone(),
            first["state"].clone(),
            first["cursor"].is_string(),
            second["done"].clone(),
            second["state"].clone(),
            second["outcome"]["type"].clone(),
            Some(second["outcome"].clone()),
            answer.contains("depth 1"),
            read["text"].as_str().unwrap().ends_with(answer.trim_end()),
            listed["sessions"][0]["session"].clone(),
        ),
        (
            json!(workspace),
            json!(false),
            json!("running"),
            true,
            json!(true),
            json!("completed"),
            json!("answered"),
            observed,
            true,
            true,
            json!(session),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
    assert_eq!(
        (
            prompted,
            again["state"].clone(),
            // The fixture answers with the tool's output.
            again["outcome"]["text"]
                .as_str()
                .map(|text| text.contains("again")),
        ),
        (
            Ok(json!({"session": session})),
            json!("completed"),
            Some(true)
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: no MCP tool answers approvals: the server lists exactly the seven
// session-control tools, refuses any other name, and a turn that asks for an
// approval stays waiting for a person however the host calls (#1672).
// Owner: rho mcp serve (no approval path).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_mcp_tool_answers_a_sessions_approval() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("supervised");
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    let write = json!({
        "id": "edit",
        "name": "write",
        "arguments": {"path": "note.txt", "content": "approved"},
    });

    let tools: Vec<String> = host
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect();
    let started = call(
        &host,
        "new",
        json!({"workspace": sandbox.workspace, "prompt": write.to_string()}),
    )
    .await
    .unwrap();
    let session = started["session"].clone();
    let waited = call(
        &host,
        "wait",
        json!({"session": session, "timeout_secs": 50}),
    )
    .await
    .unwrap();
    let request = waited["inputs"][0]["id"].clone();
    let answers: Vec<Result<Value, String>> =
        futures_util::future::join_all(["answer", "approve", "respond"].map(|tool| {
            call(
                &host,
                tool,
                json!({"session": session, "id": request, "answer": "allow_once"}),
            )
        }))
        .await;
    // The core's own actions take no answer either.
    let prompted = call(
        &host,
        "steer",
        json!({"session": session, "text": "allow_once", "answer": "allow_once"}),
    )
    .await;
    let client = sandbox.connect().await;
    let jobs = client.request::<JobList>(&JobListParams {}).await.unwrap();
    let state = jobs
        .jobs
        .iter()
        .find(|job| job.session.as_ref().map(SessionId::as_str) == session.as_str())
        .map(|job| job.state);

    assert_eq!(
        (
            tools,
            waited["state"].clone(),
            waited["inputs"][0]["ask"]["kind"].clone(),
            answers.iter().all(Result::is_err),
            prompted
                .unwrap_err()
                .starts_with("invalid `steer` arguments"),
            state,
            sandbox.workspace.join("note.txt").exists(),
        ),
        (
            ["list", "read", "new", "prompt", "steer", "cancel", "wait"]
                .map(str::to_owned)
                .to_vec(),
            json!("waiting_for_input"),
            json!("approval"),
            true,
            true,
            Some(JobState::WaitingForInput),
            false,
        ),
        "{answers:?}"
    );
}

// Covers: the nesting-depth and concurrent-session caps refuse a start past
// them, each naming its budget, its limit, and the value asked for; a
// running session takes steering, and cancelling it frees its slot (#1672).
// Owner: session-control core caps, through rho mcp serve.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_caps_refuse_starts_past_their_limits() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let deep = stdio_server(&sandbox, "tool-call", &[], &[("RHO_SESSION_DEPTH", "3")]).await;
    let busy = stdio_server(&sandbox, "tool-call", &["--max-sessions", "1"], &[]).await;
    let new = |prompt: String| json!({"workspace": sandbox.workspace, "prompt": prompt});

    let too_deep = call(&deep, "new", new("hello".into())).await;
    let running = call(&busy, "new", new(bash(BLOCKED))).await.unwrap();
    let session = running["session"].clone();
    let too_many = call(&busy, "new", new("hello".into())).await;
    let steered = call(&busy, "steer", json!({"session": session, "text": "hurry"})).await;
    let cancelled = call(&busy, "cancel", json!({"session": session})).await;
    let settled = call(
        &busy,
        "wait",
        json!({"session": session, "timeout_secs": 50}),
    )
    .await
    .unwrap();
    let freed = call(&busy, "new", new("hello".into())).await;

    assert_eq!(
        (
            too_deep,
            too_many,
            steered.map(|steered| steered["id"].is_string()),
            cancelled,
            settled["state"].clone(),
            freed.map(|_| ()),
        ),
        (
            Err("session depth budget: limit 3, asked 4; a session at depth 3 may not start or prompt another".into()),
            Err("concurrent sessions budget for this server's caller: limit 1, asked 2".into()),
            Ok(true),
            Ok(json!({})),
            json!("cancelled"),
            Ok(()),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

/// `rho mcp serve --http` on a free loopback port; returns the process and
/// its endpoint URL once it listens.
async fn http_server(sandbox: &Sandbox) -> (tokio::process::Child, String) {
    let mut command = tokio::process::Command::from(sandbox.rho("tool-call"));
    command
        .args(["mcp", "serve", "--http", "127.0.0.1:0"])
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut server = command.spawn().unwrap();
    let mut lines = BufReader::new(server.stderr.take().unwrap()).lines();
    let line = tokio::time::timeout(DEADLINE, lines.next_line())
        .await
        .expect("the server listens")
        .unwrap()
        .expect("the server says where it listens");
    let url = line
        .strip_prefix("rho mcp serve listening on ")
        .unwrap_or_else(|| panic!("unexpected first line: {line}"))
        .to_owned();
    (server, url)
}

/// Creates token `name` with `args` and returns its secret.
fn rotate(sandbox: &Sandbox, name: &str, args: &[&str]) -> String {
    let output = sandbox
        .rho("tool-call")
        .args(["mcp", "token", "rotate", name])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// The status a raw `initialize` POST to `url` gets with these headers.
async fn initialize_status(url: &str, headers: &[(&str, &str)]) -> u16 {
    let address = url
        .strip_prefix("http://")
        .and_then(|rest| rest.strip_suffix("/mcp"))
        .unwrap();
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "raw", "version": "0"},
        },
    })
    .to_string();
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let mut buffer = [0_u8; 512];
    // The status line is all this needs; an event stream may stay open.
    while !response.windows(2).any(|window| window == b"\r\n") {
        let read = tokio::time::timeout(DEADLINE, stream.read(&mut buffer))
            .await
            .expect("the server answers")
            .unwrap();
        assert!(read > 0, "the server closed before answering");
        response.extend_from_slice(&buffer[..read]);
    }
    let status = String::from_utf8_lossy(&response);
    status.split(' ').nth(1).unwrap().parse().unwrap()
}

/// An MCP client of the HTTP server at `url` with `token`.
async fn http_client(url: &str, token: &str) -> McpClient {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token.to_owned()),
    );
    ().serve(transport).await.expect("the client initializes")
}

// Covers: the HTTP transport refuses a missing or wrong bearer token and a
// browser origin it does not allow, and the core refuses a read-only token's
// write and a workspace outside a token's list, naming that list (#1672).
// Owner: rho mcp serve --http auth.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_refuses_what_its_tokens_do_not_allow() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let sandbox = Sandbox::new();
    let allowed = sandbox.workspace.display().to_string();
    let reader = rotate(&sandbox, "reader", &["--workspace", &allowed]);
    let writer = rotate(
        &sandbox,
        "writer",
        &["--scope", "write", "--workspace", &allowed],
    );
    let (_server, url) = http_server(&sandbox).await;
    let bearer = |token: &str| format!("Bearer {token}");

    let statuses = [
        initialize_status(&url, &[]).await,
        initialize_status(&url, &[("Authorization", "Bearer rho_mcp_wrong")]).await,
        initialize_status(
            &url,
            &[
                ("Authorization", &bearer(&reader)),
                ("Origin", "https://evil.example"),
            ],
        )
        .await,
        initialize_status(&url, &[("Authorization", &bearer(&reader))]).await,
    ];
    let read_only = http_client(&url, &reader).await;
    let listed = call(&read_only, "list", json!({})).await;
    let read_only_new = call(
        &read_only,
        "new",
        json!({"workspace": sandbox.workspace, "prompt": "hello"}),
    )
    .await;
    let outside = elsewhere(&sandbox);
    let write = http_client(&url, &writer).await;
    let outside_new = call(
        &write,
        "new",
        json!({"workspace": outside, "prompt": "hello"}),
    )
    .await;

    assert_eq!(
        (statuses, listed, read_only_new, outside_new),
        (
            [401, 401, 403, 200],
            Ok(json!({"sessions": [], "more": false})),
            Err("token \"reader\" has read access; `new` needs write".into()),
            Err(format!(
                "token \"writer\" may not reach workspace {}; it may reach: {allowed}",
                outside.display()
            )),
        )
    );
}
