//! A person's own sessions through the real binary: an HTTP token reaches
//! every session in its workspaces, whoever opened it, and none outside
//! them, and `list` names a session by its title after the daemon unloaded
//! it.

use pretty_assertions::assert_eq;
use rho_protocol::{methods::SessionRename, SessionRenameParams};
use serde_json::json;

use super::{
    a_persons_running_session, bash, call, elsewhere, http_client, http_server, rotate,
    stdio_server, Sandbox,
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
// as a person's, apart from the one the token started, to a token whose
// workspaces reach it, which steers and cancels its turn and prompts its
// next one; a token whose workspaces do not reach it is shown nothing and
// refused, naming what it may reach.
// Owner: rho mcp serve --http token reach, over the session-control core.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_drives_a_session_a_person_opened_within_its_workspaces() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let person = sandbox.start_daemon("tool-call").await;
    let session = a_persons_running_session(&sandbox, &person).await;
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
    let own = call(
        &bot,
        "new",
        json!({"workspace": sandbox.workspace, "prompt": bash("echo mine")}),
    )
    .await
    .unwrap();
    call(
        &bot,
        "wait",
        json!({"session": own["session"], "timeout_secs": 50}),
    )
    .await
    .unwrap();
    // Each session a `list` returned, with who opened it and its state.
    let listed = |list: serde_json::Value| -> Vec<_> {
        list["sessions"]
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
            .collect()
    };

    let persons = call(&bot, "list", json!({"opened_by": "person"}))
        .await
        .unwrap();
    let agents = call(&bot, "list", json!({"opened_by": "agent"}))
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

    assert_eq!(
        (
            listed(persons),
            listed(agents),
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
            vec![(own["session"].clone(), json!("agent"), json!("completed"))],
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
