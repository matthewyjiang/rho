//! `fast_mode` over ACP: a switch for hosts that advertise
//! `clientCapabilities.session.configOptions.boolean`, a two-value select for
//! the rest.

use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::AcpServer;

/// A fixture home on xAI OAuth `grok-4.7`, which supports fast mode.
fn server() -> (TempDir, AcpServer) {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        r#"provider = "xai"
model = "grok-4.7"
auth = "xai-oauth"

[web_search]
hosted = false
provider = "disabled"
"#,
    )
    .unwrap();
    let project = home.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let server = AcpServer::spawn(home.path(), &project, "fixed", &project);
    (home, server)
}

/// The `[type, currentValue]` of the advertised `fast_mode` option.
fn fast_mode(options: &Value) -> Value {
    let option = options
        .as_array()
        .unwrap()
        .iter()
        .find(|option| option["id"] == "fast_mode")
        .unwrap_or_else(|| panic!("no fast_mode in {options:#}"));
    json!([option["type"], option["currentValue"]])
}

// Covers: the boolean capability negotiated at initialize must reach the
// session, so a boolean host gets and sets fast mode as a switch while a
// baseline host keeps the select (ACP forbids booleans to it), and each
// rejects the other host's value shape with InvalidParams.
// Owner: ACP session config options, over the real stdio server
#[test]
fn fast_mode_is_a_switch_only_for_hosts_that_advertise_boolean_options() {
    for (label, capabilities, turn_on, wrong_shape, off, on) in [
        (
            "boolean host",
            json!({"session": {"configOptions": {"boolean": {}}}}),
            json!({"type": "boolean", "value": true}),
            json!({"value": "on"}),
            json!(["boolean", false]),
            json!(["boolean", true]),
        ),
        (
            "baseline host",
            json!({}),
            json!({"value": "on"}),
            json!({"type": "boolean", "value": true}),
            json!(["select", "off"]),
            json!(["select", "on"]),
        ),
    ] {
        let (home, mut server) = server();
        server
            .request(
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": capabilities}),
            )
            .0
            .unwrap();
        let created = server
            .request(
                "session/new",
                json!({"cwd": home.path().join("project"), "mcpServers": []}),
            )
            .0
            .unwrap();
        let session_id = created["sessionId"].clone();
        let mut set = |value: &Value| {
            let mut params = json!({"sessionId": session_id, "configId": "fast_mode"});
            params
                .as_object_mut()
                .unwrap()
                .extend(value.as_object().unwrap().clone());
            server.request("session/set_config_option", params).0
        };
        let turned_on = set(&turn_on).unwrap();
        let rejected = set(&wrong_shape).unwrap_err();
        assert_eq!(
            (
                fast_mode(&created["configOptions"]),
                fast_mode(&turned_on["configOptions"]),
                rejected["code"].clone(),
            ),
            (off, on, json!(-32602)),
            "{label}"
        );
    }
}
