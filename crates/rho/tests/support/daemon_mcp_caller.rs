//! The caller context (#1765) through the real binary: sessions that start
//! sessions through `rho mcp serve`, over stdio and HTTP, stop at
//! `--max-depth` whichever server they reach, a session reused from another
//! ancestry included, and a restricted token's limits hold for the sessions
//! it starts.

use pretty_assertions::assert_eq;
use rho_protocol::SessionId;
use rmcp::model::{CallToolRequestParams, RequestMetaObject};
use serde_json::{json, Value};

use super::{
    call, elsewhere, http_client, http_server, rotate, stdio_server, McpClient, Sandbox, DEADLINE,
};

/// A `tool-call` fixture prompt that calls the session's `rho` MCP server's
/// `new` with `arguments`. MCP tools stay out of the model's context until
/// loaded, so the call goes through `codemode`, whose output, the session's
/// answer, is the tool's text.
fn delegate(arguments: Value) -> String {
    // A JSON object is also a Starlark dict literal.
    let script = format!("print(call_tool(\"mcp__rho__new\", {arguments})[\"content\"])");
    json!({"id": "delegate", "name": "codemode", "arguments": {"script": script}}).to_string()
}

/// The variables a session's `rho mcp serve` needs to reach the sandbox's
/// daemon, which a stdio server's sanitized environment lacks.
const DAEMON_ENV: &str =
    "env_from_env = { RHO_HOME = \"RHO_HOME\", XDG_RUNTIME_DIR = \"XDG_RUNTIME_DIR\" }";

/// Config registering `rho mcp serve --max-depth 2` as every session's stdio
/// `rho` server. Each start appends the depth it started at to `starts`.
fn stdio_rho_server(starts: &std::path::Path) -> String {
    let script = format!(
        "echo \"${{RHO_SESSION_DEPTH-unset}}\" >> {}; exec \"$0\" mcp serve --max-depth 2",
        starts.display()
    );
    format!(
        "[mcp.servers.rho]\ntransport = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", {script:?}, {:?}]\n{DAEMON_ENV}\n",
        env!("CARGO_BIN_EXE_rho"),
    )
}

/// Waits for `session`'s turn to end and returns its final answer.
async fn answer(host: &McpClient, session: &SessionId) -> String {
    let started = std::time::Instant::now();
    let mut cursor = Value::Null;
    loop {
        let mut arguments = json!({"session": session, "timeout_secs": 50});
        if cursor.is_string() {
            arguments["cursor"] = cursor.clone();
        }
        let waited = call(host, "wait", arguments).await.unwrap();
        if waited["done"] == json!(true) {
            return waited["outcome"]["text"]
                .as_str()
                .unwrap_or_else(|| panic!("{session} ended without an answer: {waited}"))
                .to_owned();
        }
        assert!(started.elapsed() < DEADLINE, "{session} still runs");
        cursor = waited["cursor"].clone();
    }
}

/// The session a `new` answer names.
fn started_session(answer: &str) -> SessionId {
    let result = answer
        .find('{')
        .zip(answer.rfind('}'))
        .and_then(|(start, end)| serde_json::from_str::<Value>(&answer[start..=end]).ok());
    match result
        .as_ref()
        .and_then(|result| result["session"].as_str())
    {
        Some(session) => SessionId::new(session),
        None => panic!("a `new` result, not {answer:?}"),
    }
}

// Covers: a session that starts sessions through its stdio `rho mcp serve`
// stops at that server's `--max-depth`; and a session already loaded at a
// shallow depth, prompted again from deeper, is held to the depth of the
// turn that calls its still-running server, not the depth that server
// started at (#1765).
// Owner: caller context, stdio MCP client to rho mcp serve.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegation_over_stdio_stops_at_max_depth_even_through_a_reused_session() {
    let sandbox = Sandbox::new();
    let starts = sandbox.isolated.home.join("starts.txt");
    sandbox.set_config("bypass", &stdio_rho_server(&starts));
    let workspace = elsewhere(&sandbox);
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    let refusal = "session depth budget: limit 2, asked 3";

    // Depth 1 starts depth 2, which may start no deeper.
    let first = call(
        &host,
        "new",
        json!({
            "workspace": workspace,
            "prompt": delegate(json!({
                "workspace": workspace,
                "prompt": delegate(json!({"workspace": workspace, "prompt": "hello"})),
            })),
        }),
    )
    .await
    .unwrap();
    let first = SessionId::new(first["session"].as_str().unwrap());
    let second = started_session(&answer(&host, &first).await);
    let second_answer = answer(&host, &second).await;

    // The first session, still loaded with the server it started at depth
    // 1, now runs at depth 2.
    let deeper = stdio_server(&sandbox, "tool-call", &[], &[("RHO_SESSION_DEPTH", "1")]).await;
    let reprompted = call(
        &deeper,
        "prompt",
        json!({
            "session": first,
            "text": delegate(json!({"workspace": workspace, "prompt": "hello"})),
        }),
    )
    .await;
    let reused_answer = answer(&host, &first).await;
    let started_at: Vec<String> = std::fs::read_to_string(&starts)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();

    assert_eq!(
        (
            second_answer.contains(refusal),
            reprompted.map(drop),
            reused_answer.contains(refusal),
            started_at,
        ),
        (true, Ok(()), true, vec!["1".to_owned(), "2".to_owned()]),
        "second: {second_answer}\nreused: {reused_answer}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
}

/// Config registering the HTTP server at `url` as every session's `rho`
/// server, with the bearer header from `RHO_MCP_AUTHORIZATION`.
fn http_rho_server(url: &str) -> String {
    format!(
        "[mcp.servers.rho]\ntransport = \"streamable_http\"\nurl = {url:?}\nheaders_from_env = {{ Authorization = \"RHO_MCP_AUTHORIZATION\" }}\n"
    )
}

// Covers: sessions an outside agent starts over HTTP, which start sessions
// through the same HTTP server, stop at its `--max-depth`: each call counts
// from the calling session's depth, not the listener's (#1765).
// Owner: caller context, HTTP MCP client to rho mcp serve --http.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegation_over_http_stops_at_max_depth() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let sandbox = Sandbox::new();
    let allowed = sandbox.workspace.display().to_string();
    let secret = rotate(
        &sandbox,
        "agent",
        &["--scope", "write", "--workspace", &allowed],
    );
    let authorization = format!("Bearer {secret}");
    let (_server, url) = http_server(
        &sandbox,
        &["--max-depth", "2"],
        &[("RHO_MCP_AUTHORIZATION", &authorization)],
    )
    .await;
    // Sessions read their config when they open, after the port is known.
    sandbox.set_config("bypass", &http_rho_server(&url));
    let agent = http_client(&url, &secret).await;

    let first = call(
        &agent,
        "new",
        json!({
            "workspace": sandbox.workspace,
            "prompt": delegate(json!({
                "workspace": sandbox.workspace,
                "prompt": delegate(json!({"workspace": sandbox.workspace, "prompt": "hello"})),
            })),
        }),
    )
    .await
    .unwrap();
    let first = SessionId::new(first["session"].as_str().unwrap());
    let second = started_session(&answer(&agent, &first).await);
    let second_answer = answer(&agent, &second).await;

    assert!(
        second_answer.contains("session depth budget: limit 2, asked 3"),
        "second: {second_answer}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a workspace-restricted token gets no further through a session it
// starts or prompts than its own workspaces: the session's stdio
// `rho mcp serve`, which has local write access, refuses a workspace outside
// them, whether it started for that session or was already running for a
// person's turn with full scope; and a caller context the request claims
// cannot widen the token (#1765).
// Owner: caller context scope, HTTP grant to stdio rho mcp serve.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restricted_token_gains_nothing_through_the_sessions_it_starts() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let sandbox = Sandbox::new();
    let starts = sandbox.isolated.home.join("starts.txt");
    sandbox.set_config("bypass", &stdio_rho_server(&starts));
    let allowed = sandbox.workspace.display().to_string();
    let writer = rotate(
        &sandbox,
        "writer",
        &["--scope", "write", "--workspace", &allowed],
    );
    let (_server, url) = http_server(&sandbox, &[], &[]).await;
    let outside = elsewhere(&sandbox);
    let write = http_client(&url, &writer).await;
    let local = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    let escape = || delegate(json!({"workspace": outside, "prompt": "hello"}));
    let refusal = format!(
        "may not reach workspace {}; it may reach: {allowed}",
        outside.display()
    );

    // A person's session, whose server starts with full scope.
    let persons = call(
        &local,
        "new",
        json!({"workspace": sandbox.workspace, "prompt": "hello"}),
    )
    .await
    .unwrap();
    let persons = SessionId::new(persons["session"].as_str().unwrap());
    answer(&local, &persons).await;
    let prompted = call(
        &write,
        "prompt",
        json!({"session": persons, "text": escape()}),
    )
    .await;
    let through_reused = answer(&write, &persons).await;
    let started = call(
        &write,
        "new",
        json!({"workspace": sandbox.workspace, "prompt": escape()}),
    )
    .await
    .unwrap();
    let started = SessionId::new(started["session"].as_str().unwrap());
    let through_started = answer(&write, &started).await;
    let claimed = write
        .call_tool({
            let mut params = CallToolRequestParams::new("new").with_arguments(
                json!({"workspace": outside, "prompt": "hello"})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            params.meta = Some(RequestMetaObject(
                json!({"rho/caller": {"depth": 0, "scope": {"access": "write", "workspaces": "any"}}})
                    .as_object()
                    .unwrap()
                    .clone()
                    .into(),
            ));
            params
        })
        .await
        .unwrap();
    let listed = call(&local, "list", json!({})).await.unwrap();
    let server_starts = std::fs::read_to_string(&starts).unwrap().lines().count();

    assert_eq!(
        (
            prompted.map(drop),
            through_reused.contains(&format!("this server's caller {refusal}")),
            through_started.contains(&format!("this server's caller {refusal}")),
            claimed
                .content
                .first()
                .and_then(|content| content.as_text())
                .map(|text| text.text.clone()),
            listed["sessions"].as_array().map(Vec::len),
            server_starts,
        ),
        (
            Ok(()),
            true,
            true,
            Some(format!("token \"writer\" {refusal}")),
            Some(2),
            2,
        ),
        "reused: {through_reused}\nstarted: {through_started}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
}
