//! Cumulative session cost in ACP `usage_update`, read from the usage ledger
//! the fixture provider's priced responses are recorded into.

use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::AcpServer;

/// Cost the fixture reports for every response, in USD micros.
const RESPONSE_COST_USD_MICROS: &str = "1250";

/// A fixture home; in `fixed` mode every prompt is one provider request.
fn home() -> TempDir {
    let home = TempDir::new().unwrap();
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
    std::fs::create_dir_all(home.path().join("project")).unwrap();
    home
}

/// An initialized server whose responses each cost `cost` USD micros, or
/// carry no price when `None`.
fn server(home: &TempDir, cost: Option<&str>) -> AcpServer {
    let project = home.path().join("project");
    let mut command = AcpServer::command(home.path(), &project, "fixed", &project);
    if let Some(cost) = cost {
        command.env("RHO_AUTOMATION_TEST_COST_USD_MICROS", cost);
    }
    let mut server = AcpServer::from_command(&mut command);
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    server
}

/// The `cost` of every `usage_update` one prompt sent, in order.
fn prompt_costs(server: &mut AcpServer, session_id: &str) -> Vec<Value> {
    let (reply, updates) = server.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "hello"}]}),
    );
    assert_eq!(reply.unwrap()["stopReason"], "end_turn");
    updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "usage_update")
        .map(|update| update["cost"].clone())
        .collect()
}

fn usd(amount: f64) -> Value {
    json!({"amount": amount, "currency": "USD"})
}

// Covers: `usage_update.cost` is the session's cumulative cost; after a
// server restart and `session/load` it continues from the saved turns
// instead of resetting or counting them twice; and it is omitted, never a
// fake 0, while no request has a price.
// Owner: ACP session signals, over the real stdio server
#[test]
fn usage_updates_report_cumulative_session_cost_across_load() {
    for (label, cost, first, after_load) in [
        (
            "priced",
            Some(RESPONSE_COST_USD_MICROS),
            // Step start (nothing spent yet), then turn end.
            vec![Value::Null, usd(0.00125)],
            vec![usd(0.00125), usd(0.0025)],
        ),
        (
            "unpriced",
            None,
            vec![Value::Null, Value::Null],
            vec![Value::Null, Value::Null],
        ),
    ] {
        let home = home();
        let project = home.path().join("project");
        let mut server = server(&home, cost);
        let created = server
            .request("session/new", json!({"cwd": project, "mcpServers": []}))
            .0
            .unwrap();
        let session_id = created["sessionId"].as_str().unwrap().to_owned();
        assert_eq!(prompt_costs(&mut server, &session_id), first, "{label}");

        drop(server);
        let mut server = self::server(&home, cost);
        server
            .request(
                "session/load",
                json!({"sessionId": session_id, "cwd": project, "mcpServers": []}),
            )
            .0
            .unwrap();
        assert_eq!(
            prompt_costs(&mut server, &session_id),
            after_load,
            "{label}"
        );
    }
}
