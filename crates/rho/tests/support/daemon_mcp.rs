//! `rho mcp serve` through the real binary: an MCP host drives a daemon
//! session in another workspace over stdio while a second daemon client
//! follows it, no tool can answer the session's approvals, and the core's
//! caps refuse with their budgets.

use std::{path::PathBuf, process::Stdio};

use pretty_assertions::assert_eq;
use rho_protocol::{methods::JobList, JobListParams, JobState, SessionEvent, SessionId};
use rmcp::{
    model::CallToolRequestParams, service::RunningService, transport::TokioChildProcess,
    RoleClient, ServiceExt,
};
use serde_json::{json, Value};

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
