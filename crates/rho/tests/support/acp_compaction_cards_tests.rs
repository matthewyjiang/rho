//! Compaction reported over ACP: `compaction_update` cards for hosts that
//! advertise `clientCapabilities.session.compaction`, an agent message for
//! the rest.

use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::AcpServer;

/// A fixture home whose model has `window` usable tokens, so a small window
/// makes every prompt compact automatically.
fn server(window: Option<u32>) -> (TempDir, AcpServer) {
    let home = TempDir::new().unwrap();
    let rho_home = home.path().join(".rho");
    std::fs::create_dir_all(&rho_home).unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        r#"provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"

[web_search]
hosted = false
provider = "disabled"
"#,
    )
    .unwrap();
    if let Some(window) = window {
        std::fs::write(
            rho_home.join("models.toml"),
            format!("[models.\"xai/grok-fixture\"]\nusable_context_window = {window}\n"),
        )
        .unwrap();
    }
    let project = home.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let server = AcpServer::spawn(home.path(), &project, "fixed", &project);
    (home, server)
}

fn start_session(
    server: &mut AcpServer,
    client_capabilities: Value,
    cwd: &std::path::Path,
) -> String {
    server
        .request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": client_capabilities}),
        )
        .0
        .unwrap();
    let created = server
        .request("session/new", json!({"cwd": cwd, "mcpServers": []}))
        .0
        .unwrap();
    created["sessionId"].as_str().unwrap().to_owned()
}

fn prompt(server: &mut AcpServer, session_id: &str, text: &str) -> Vec<Value> {
    let (reply, updates) = server.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
    );
    assert_eq!(reply.unwrap()["stopReason"], "end_turn");
    updates
}

/// The `(status, summary present)` sequence of the one card in `updates`.
fn card_lifecycle(updates: &[Value]) -> Vec<(String, bool)> {
    let cards = updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "compaction_update")
        .collect::<Vec<_>>();
    let ids = cards
        .iter()
        .map(|card| card["compactionId"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(ids.len() <= 1, "one card per compaction: {cards:#?}");
    cards
        .iter()
        .map(|card| {
            (
                card["status"].as_str().unwrap().to_owned(),
                !card["summary"].is_null(),
            )
        })
        .collect()
}

fn agent_text(updates: &[Value]) -> Vec<&str> {
    updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update["content"]["text"].as_str())
        .collect()
}

// Covers: `/compact` must report as one compaction card that opens in
// progress and completes, with no status text posing as the retained
// summary, when the host advertised compaction, and as the agent message otherwise (ACP forbids the card
// then), so the capability negotiated at initialize reaches the session.
// Owner: ACP session updates, over the real stdio server
#[test]
fn manual_compaction_reports_a_card_only_to_hosts_that_advertise_it() {
    for (label, capabilities, card, messages) in [
        (
            "compaction host",
            json!({"session": {"compaction": {}}}),
            vec![
                ("in_progress".to_owned(), false),
                ("completed".to_owned(), false),
            ],
            0,
        ),
        ("baseline host", json!({}), Vec::new(), 1),
    ] {
        let (home, mut server) = server(None);
        let cwd = home.path().join("project");
        let session_id = start_session(&mut server, capabilities, &cwd);
        prompt(&mut server, &session_id, "hello");
        let updates = prompt(&mut server, &session_id, "/compact");
        assert_eq!(
            (card_lifecycle(&updates), agent_text(&updates).len()),
            (card, messages),
            "{label}: {updates:#?}"
        );
    }
}

// Covers: compaction the runtime starts inside a turn must reach a
// compaction host as one card that opens and completes, not stay invisible.
// Owner: ACP session updates, over the real stdio server
#[test]
fn automatic_compaction_reports_a_card() {
    let (home, mut server) = server(Some(1_000));
    let cwd = home.path().join("project");
    let session_id = start_session(&mut server, json!({"session": {"compaction": {}}}), &cwd);
    let mut lifecycles = Vec::new();
    for text in ["hello", "again"] {
        let updates = prompt(&mut server, &session_id, text);
        lifecycles.push(card_lifecycle(&updates));
    }
    assert!(
        lifecycles.contains(&vec![
            ("in_progress".to_owned(), false),
            ("completed".to_owned(), false)
        ]),
        "{lifecycles:#?}"
    );
}
