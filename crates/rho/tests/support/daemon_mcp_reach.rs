//! A person's own sessions through the real binary: an HTTP token reaches
//! every session in its workspaces, whoever opened it, and none outside
//! them, and `list` names a session by its title after the daemon unloaded
//! it.

use pretty_assertions::assert_eq;
use rho_protocol::{methods::SessionRename, SessionRenameParams};
use serde_json::json;

use super::{
    bash, bash_call, call, elsewhere, http_client, http_server, rotate, stdio_server, Sandbox,
    BLOCKED,
};

// Covers: `list` names a session by its saved title once the daemon
// unloaded it, when the daemon's job list carries no title for it.
// Owner: session-control list, over the daemon's catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_titles_a_session_the_daemon_unloaded() {
    let sandbox = Sandbox::new();
    let person = sandbox
        .start_daemon_with("tool-call", &["--unload-idle-after", "200ms"])
        .await;
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    let session = sandbox.new_session(&person).await;
    sandbox.prompt(&person, &session).await.unwrap();
    // The turn saves the session, which a rename needs.
    call(
        &host,
        "wait",
        json!({"session": session, "timeout_secs": 50}),
    )
    .await
    .unwrap();
    person
        .request::<SessionRename>(&SessionRenameParams {
            session: session.clone(),
            title: "Fix the flaky test".to_owned(),
        })
        .await
        .unwrap();
    sandbox.wait_unloaded(&session, 1).await;

    let listed = call(&host, "list", json!({})).await.unwrap();

    let titles: Vec<_> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (entry["session"].clone(), entry["title"].clone()))
        .collect();
    assert_eq!(
        titles,
        vec![(json!(session), json!("Fix the flaky test"))],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a session a person opened from a frontend, outside MCP, is listed
// as a person's to a token whose workspaces reach it, which steers and
// cancels its turn and prompts its next one; a token whose workspaces do
// not reach it is shown nothing and refused, naming what it may reach.
// Owner: rho mcp serve --http token reach, over the session-control core.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_drives_a_session_a_person_opened_within_its_workspaces() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let person = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session(&person).await;
    sandbox
        .prompt_tool_call(
            &person,
            &session,
            &bash_call(&format!("touch started; {BLOCKED}")),
        )
        .await;
    // Steer only once bash runs: a steer before the first model request
    // joins it, and the fixture then answers that text and ends the turn.
    sandbox
        .poll_until("the person's bash call to start", async || {
            sandbox.workspace.join("started").exists().then_some(())
        })
        .await;
    let outside = elsewhere(&sandbox).display().to_string();
    let everywhere = rotate(&sandbox, "bot", &["--scope", "write", "--workspace", "/"]);
    let narrow = rotate(
        &sandbox,
        "narrow",
        &["--scope", "write", "--workspace", &outside],
    );
    let (_server, url) = http_server(&sandbox, &[], &[]).await;
    let bot = http_client(&url, &everywhere).await;
    let outsider = http_client(&url, &narrow).await;
    let steer = json!({"session": session, "text": "hurry"});

    let found = call(&bot, "list", json!({"opened_by": "person"}))
        .await
        .unwrap();
    let hidden = call(&outsider, "list", json!({})).await;
    let refused = call(&outsider, "steer", steer.clone()).await;
    let steered = call(&bot, "steer", steer).await;
    let cancelled = call(&bot, "cancel", json!({"session": session})).await;
    let settled = call(
        &bot,
        "wait",
        json!({"session": session, "timeout_secs": 50}),
    )
    .await
    .unwrap();
    let prompted = call(
        &bot,
        "prompt",
        json!({"session": session, "text": bash("echo adopted")}),
    )
    .await;
    let again = call(
        &bot,
        "wait",
        json!({"session": session, "cursor": settled["cursor"], "timeout_secs": 50}),
    )
    .await
    .unwrap();

    let entries: Vec<_> = found["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["session"].clone(),
                entry["opened_by"].clone(),
                entry["state"].clone(),
            )
        })
        .collect();
    assert_eq!(
        (
            entries,
            hidden,
            refused,
            steered.map(|steered| steered["id"].is_string()),
            cancelled,
            settled["state"].clone(),
            prompted,
            again["state"].clone(),
            // The fixture answers with the tool's output.
            again["outcome"]["text"]
                .as_str()
                .map(|text| text.contains("adopted")),
        ),
        (
            vec![(json!(session), json!("person"), json!("running"))],
            Ok(json!({"sessions": [], "more": false})),
            Err(format!(
                "token \"narrow\" may not reach workspace {}; it may reach: {outside}",
                sandbox.resolved_workspace().display()
            )),
            Ok(true),
            Ok(json!({})),
            json!("cancelled"),
            Ok(json!({"session": session})),
            json!("completed"),
            Some(true),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
