//! A hub through the real binary: one machine's `rho mcp serve` drives a
//! session on another machine's `rho mcp serve --http`, added with `rho
//! mcp remote add` from the token that machine issued it, and watches it,
//! POSTing its news to the hub's own webhook; a hub's caller drives a session
//! a person opened on the remote; and hubs that list each other
//! serve each other's calls without passing them around. Each machine is a
//! sandbox of its own, with its own daemon and credential store.

use std::{io::Write, process::Stdio};

use pretty_assertions::assert_eq;
use rmcp::model::CallToolRequestParams;
use serde_json::{json, Value};

use super::{
    a_persons_running_session, bash, call, http_client, http_server, rotate, stdio_server,
    webhooks::{add_webhook, receiver},
    Sandbox, DEADLINE,
};

/// A machine serving its sessions over HTTP as `name`, with a write token
/// for the hub on its workspace: its URL and the token's secret. The
/// server stops when the returned child drops.
async fn remote_machine(machine: &Sandbox, name: &str) -> (tokio::process::Child, String, String) {
    machine.set_permission_mode("bypass");
    let workspace = machine.workspace.display().to_string();
    let secret = rotate(
        machine,
        "hub",
        &["--scope", "write", "--workspace", &workspace],
    );
    let (server, url) = http_server(machine, &["--name", name], &[]).await;
    (server, url, secret)
}

/// Adds remote `name` at `url` on `hub`, `secret` piped on stdin as `ssh`
/// would pipe it from the remote's `rho mcp token rotate`.
fn add_remote(hub: &Sandbox, name: &str, url: &str, secret: &str) {
    let mut add = hub
        .rho("tool-call")
        .args(["mcp", "remote", "add", name, "--url", url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    add.stdin
        .take()
        .unwrap()
        .write_all(format!("{secret}\n").as_bytes())
        .unwrap();
    let output = add.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
}

/// The servers a fanned-out `list` names, each with its sessions' ids; a
/// server that could not list fails the test with its error.
fn listed(list: &Value) -> Vec<(Value, Vec<Value>)> {
    listed_as(list, |entry| entry["session"].clone())
}

/// The servers a fanned-out `list` names, each with what `shown` takes of
/// its sessions' entries; a server that could not list fails the test with
/// its error.
fn listed_as<T>(list: &Value, shown: impl Fn(&Value) -> T) -> Vec<(Value, Vec<T>)> {
    list["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|server| {
            let sessions = server["sessions"]
                .as_array()
                .unwrap_or_else(|| panic!("a server that listed its sessions, not {server}"))
                .iter()
                .map(&shown)
                .collect();
            (server["server"].clone(), sessions)
        })
        .collect()
}

// Covers: a stdio hub with a remote starts a session on that remote by
// `server`, follows it to its answer there, lists every machine's sessions
// when `list` names none, and refuses a call that names no server, listing
// the servers there are.
// Owner: rho mcp serve hub routing, through the actual binary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hub_drives_a_session_on_a_remote_machine() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let remote = Sandbox::new();
    let hub = Sandbox::new();
    let (_remote_server, url, secret) = remote_machine(&remote, "lab-box").await;
    add_remote(&hub, "lab-box", &url, &secret);
    let host = stdio_server(&hub, "tool-call", &["--name", "hub"], &[]).await;

    let started = call(
        &host,
        "new",
        json!({"server": "lab-box", "workspace": remote.workspace, "prompt": bash("echo on lab-box")}),
    )
    .await
    .unwrap();
    let session = started["session"].clone();
    let waited = call(
        &host,
        "wait",
        json!({"server": "lab-box", "session": session, "timeout_secs": 50}),
    )
    .await
    .unwrap();
    let every = call(&host, "list", json!({})).await.unwrap();
    let unnamed = call(&host, "wait", json!({"session": session})).await;

    assert_eq!(
        (
            started["server"].clone(),
            waited["server"].clone(),
            waited["state"].clone(),
            waited["outcome"]["text"]
                .as_str()
                .map(|text| text.contains("on lab-box")),
            listed(&every),
            unnamed,
        ),
        (
            json!("lab-box"),
            json!("lab-box"),
            json!("completed"),
            Some(true),
            vec![
                (json!("hub"), Vec::new()),
                (json!("lab-box"), vec![session])
            ],
            Err("`wait` needs `server`, the machine to act on: one of hub, lab-box".to_owned()),
        ),
        "remote log:\n{}",
        remote.isolated.daemon_log()
    );
}

// Covers: a hub's caller finds a session a person opened on a remote,
// apart from the one the hub started there, the hub passing `list`'s
// filter on, and steers and cancels it there by `server`, through the
// token the remote issued the hub.
// Owner: rho mcp serve hub routing, through the actual binary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hub_drives_a_session_a_person_opened_on_a_remote() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let remote = Sandbox::new();
    let hub = Sandbox::new();
    remote.set_permission_mode("bypass");
    let person = remote.start_daemon("tool-call").await;
    let session = a_persons_running_session(&remote, &person).await;
    let (_remote_server, url, secret) = remote_machine(&remote, "lab-box").await;
    add_remote(&hub, "lab-box", &url, &secret);
    let host = stdio_server(&hub, "tool-call", &["--name", "hub"], &[]).await;
    let on_remote = |session: &Value, mut arguments: Value| {
        arguments["server"] = json!("lab-box");
        arguments["session"] = session.clone();
        arguments
    };
    let own = call(
        &host,
        "new",
        json!({"server": "lab-box", "workspace": remote.workspace, "prompt": bash("echo mine")}),
    )
    .await
    .unwrap();
    call(
        &host,
        "wait",
        on_remote(&own["session"], json!({"timeout_secs": 50})),
    )
    .await
    .unwrap();
    let session = json!(session);

    let persons = call(&host, "list", json!({"opened_by": "person"}))
        .await
        .unwrap();
    let steered = call(
        &host,
        "steer",
        on_remote(&session, json!({"text": "hurry"})),
    )
    .await
    .unwrap();
    let cancelled = call(&host, "cancel", on_remote(&session, json!({}))).await;
    let settled = call(
        &host,
        "wait",
        on_remote(&session, json!({"timeout_secs": 50})),
    )
    .await
    .unwrap();

    assert_eq!(
        (
            listed_as(&persons, |entry| (
                entry["session"].clone(),
                entry["opened_by"].clone()
            )),
            steered["id"].is_string(),
            steered["server"].clone(),
            cancelled,
            settled["state"].clone(),
        ),
        (
            vec![
                (json!("hub"), Vec::new()),
                (json!("lab-box"), vec![(session, json!("person"))]),
            ],
            true,
            json!("lab-box"),
            Ok(json!({"server": "lab-box"})),
            json!("cancelled"),
        ),
        "remote log:\n{}",
        remote.isolated.daemon_log()
    );
}

// Covers: an HTTP hub's `notify` on a session it starts on a remote
// watches it from the hub, which POSTs the settled news to the hub's own
// webhook, naming the remote as `server`; the remote has no webhook.
// Owner: rho mcp serve hub webhook watches, through the actual binary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hub_posts_a_remote_sessions_news_to_its_webhook() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let remote = Sandbox::new();
    let hub = Sandbox::new();
    let (_remote_server, url, secret) = remote_machine(&remote, "lab-box").await;
    add_remote(&hub, "lab-box", &url, &secret);
    let workspace = hub.workspace.display().to_string();
    let token = rotate(
        &hub,
        "bot",
        &["--scope", "write", "--workspace", &workspace],
    );
    let (hook, mut received) = receiver().await;
    add_webhook(&hub, "hook", &hook, "bot", "hook-secret\n");
    let (_hub_server, hub_url) = http_server(&hub, &["--name", "hub"], &[]).await;
    let host = http_client(&hub_url, &token).await;

    let started = call(
        &host,
        "new",
        json!({
            "server": "lab-box",
            "workspace": remote.workspace,
            "prompt": bash("echo done"),
            "notify": "hook",
        }),
    )
    .await
    .unwrap();
    let (authorization, body) = tokio::time::timeout(DEADLINE, received.recv())
        .await
        .expect("the hub POSTs the news")
        .expect("the receiver runs");

    assert_eq!(
        (
            started["watching"].clone(),
            authorization,
            body["server"].clone(),
            body["session"].clone(),
            body["state"].clone(),
            body["done"].clone(),
        ),
        (
            json!(true),
            Some("Bearer hook-secret".to_owned()),
            json!("lab-box"),
            started["session"].clone(),
            json!("completed"),
            json!(true),
        ),
        "remote log:\n{}",
        remote.isolated.daemon_log()
    );
}

// Covers: two hubs that list each other as remotes serve a forwarded call
// on their own sessions: a `list` that names no server reaches each
// machine once rather than passing between them, and a session starts on
// the other hub by `server` though that hub has remotes of its own.
// Owner: rho mcp serve hub routing, through the actual binary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hubs_that_list_each_other_serve_forwarded_calls_where_they_land() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let east = Sandbox::new();
    let west = Sandbox::new();
    let (_east_server, east_url, east_secret) = remote_machine(&east, "east").await;
    let (_west_server, west_url, west_secret) = remote_machine(&west, "west").await;
    add_remote(&east, "west", &west_url, &west_secret);
    add_remote(&west, "east", &east_url, &east_secret);
    let host = stdio_server(&east, "tool-call", &["--name", "east"], &[]).await;

    let started = call(
        &host,
        "new",
        json!({"server": "west", "workspace": west.workspace, "prompt": bash("echo on west")}),
    )
    .await
    .unwrap();
    let every = tokio::time::timeout(DEADLINE, call(&host, "list", json!({})))
        .await
        .expect("a list across hubs that list each other ends")
        .unwrap();

    assert_eq!(
        (started["server"].clone(), listed(&every)),
        (
            json!("west"),
            vec![
                (json!("east"), Vec::new()),
                (json!("west"), vec![started["session"].clone()]),
            ]
        ),
        "west log:\n{}",
        west.isolated.daemon_log()
    );
}

// Covers: a server marks an error that calling again will not change, as
// for a session its daemon does not know, so a hub's watch drops the watch
// rather than retrying; an error that may pass, as a malformed cursor here,
// goes unmarked.
// Owner: rho mcp serve error results a hub's watch reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_marks_the_errors_that_last() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let remote = Sandbox::new();
    let (_server, url, secret) = remote_machine(&remote, "lab-box").await;
    let hub = http_client(&url, &secret).await;
    let lasting = |arguments: Value| {
        let hub = &hub;
        async move {
            let Value::Object(arguments) = arguments else {
                panic!("arguments are an object");
            };
            let result = tokio::time::timeout(
                DEADLINE,
                hub.call_tool(CallToolRequestParams::new("wait").with_arguments(arguments)),
            )
            .await
            .expect("the tool answers")
            .unwrap();
            (
                result.is_error,
                result
                    .meta
                    .and_then(|meta| meta.0.get("rho/lasting").cloned()),
            )
        }
    };

    assert_eq!(
        [
            lasting(json!({"session": "no-such-session", "timeout_secs": 0})).await,
            lasting(json!({"session": "no-such-session", "cursor": "not a cursor"})).await,
        ],
        [(Some(true), Some(json!(true))), (Some(true), None)],
        "remote log:\n{}",
        remote.isolated.daemon_log()
    );
}
