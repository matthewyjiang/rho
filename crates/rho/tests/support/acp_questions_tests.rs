//! Questions over a real `rho acp` stdio connection: the `questionnaire`
//! tool through form elicitation on hosts that draw forms, and leaving plan
//! mode through a permission prompt on every host.

use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::{AcpServer, REPLY_TIMEOUT};

/// A fixture home in `mode` whose model makes the tool call each prompt
/// spells out as JSON.
fn fixture_server(mode: &str) -> (TempDir, AcpServer) {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"

[behavior]
permission_mode = "{mode}"

[web_search]
hosted = false
provider = "disabled"
"#
        ),
    )
    .unwrap();
    let project = home.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let server = AcpServer::spawn(home.path(), &project, "tool-call", &project);
    (home, server)
}

fn start_session(server: &mut AcpServer, capabilities: Value, home: &TempDir) -> String {
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
    created["sessionId"].as_str().unwrap().to_owned()
}

/// What a prompt produced: the agent requests it answered, its updates, and
/// its stop reason.
struct Exchange {
    asked: Vec<Value>,
    updates: Vec<Value>,
    stop_reason: Value,
}

/// Sends a prompt making `call` and answers each agent request with
/// `answer`, as a host would while the prompt runs.
fn prompt_answering(
    server: &mut AcpServer,
    session_id: &str,
    call: &Value,
    answer: impl Fn(&Value) -> Value,
) -> Exchange {
    server.next_id += 1;
    let id = server.next_id;
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": call.to_string()}]},
    }));
    let (mut asked, mut updates) = (Vec::new(), Vec::new());
    loop {
        let message = server
            .messages
            .recv_timeout(REPLY_TIMEOUT)
            .expect("the prompt progresses");
        match message["method"].as_str() {
            Some("session/update") => updates.push(message["params"]["update"].clone()),
            Some(_) => {
                server.send(&json!({
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "result": answer(&message),
                }));
                asked.push(message);
            }
            None if message["id"] == id => {
                return Exchange {
                    asked,
                    updates,
                    stop_reason: message["result"]["stopReason"].clone(),
                };
            }
            None => panic!("reply to an unknown request: {message}"),
        }
    }
}

fn agent_text(updates: &[Value]) -> String {
    updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update["content"]["text"].as_str())
        .collect()
}

fn ask_color() -> Value {
    json!({
        "id": "ask",
        "name": "questionnaire",
        "arguments": {"questions": [{
            "id": "color",
            "question": "Which color?",
            "choices": ["Red", "Blue"],
        }]},
    })
}

// Covers: a host advertising form elicitation gets the questionnaire as an
// `elicitation/create` form on the tool's card, and its accepted answer
// reaches the model. A host without it is never asked.
// Owner: ACP questions
#[test]
fn questionnaire_asks_through_form_elicitation_only_when_advertised() {
    let (home, mut server) = fixture_server("bypass");
    let session_id = start_session(&mut server, json!({"elicitation": {"form": {}}}), &home);
    let exchange = prompt_answering(&mut server, &session_id, &ask_color(), |request| {
        let property = &request["params"]["requestedSchema"]["properties"]["color"];
        // Answer as T3 Code does: with the last `enum` value.
        let choice = property["enum"].as_array().unwrap().last().unwrap().clone();
        json!({"action": "accept", "content": {"color": choice}})
    });
    let asked = exchange
        .asked
        .iter()
        .map(|request| {
            (
                request["method"].clone(),
                request["params"]["mode"].clone(),
                request["params"]["toolCallId"].clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        asked,
        vec![(json!("elicitation/create"), json!("form"), json!("ask"))]
    );
    assert_eq!(exchange.stop_reason, "end_turn");
    let reply = agent_text(&exchange.updates);
    assert!(
        reply.contains("Blue"),
        "the answer reaches the model: {reply}"
    );

    let (home, mut server) = fixture_server("bypass");
    let session_id = start_session(&mut server, json!({}), &home);
    let exchange = prompt_answering(&mut server, &session_id, &ask_color(), |request| {
        panic!("a host without forms must not be asked: {request}")
    });
    assert_eq!(exchange.stop_reason, "end_turn");
}

// Covers: in plan mode, `exit_plan_mode` asks through
// `session/request_permission` even on a host without forms; approval
// switches the session's mode, announces it, and runs the implementation
// turn in the same prompt
// Owner: ACP plan exit
#[test]
fn approved_plan_switches_mode_and_implements_in_the_same_prompt() {
    let (home, mut server) = fixture_server("plan");
    let session_id = start_session(&mut server, json!({}), &home);
    let call = json!({
        "id": "plan",
        "name": "exit_plan_mode",
        "arguments": {"plan": "1. Change the thing."},
    });
    let exchange = prompt_answering(
        &mut server,
        &session_id,
        &call,
        |_| json!({"outcome": {"outcome": "selected", "optionId": "allow_edits"}}),
    );

    let asked = exchange
        .asked
        .iter()
        .map(|request| {
            (
                request["method"].clone(),
                request["params"]["toolCall"]["toolCallId"].clone(),
                request["params"]["options"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|option| option["optionId"].clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        asked,
        vec![(
            json!("session/request_permission"),
            json!("plan"),
            vec![
                json!("allow_edits"),
                json!("supervised"),
                json!("bypass"),
                json!("keep_planning"),
            ],
        )]
    );
    let modes = exchange
        .updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "current_mode_update")
        .map(|update| update["currentModeId"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        (modes, exchange.stop_reason),
        (vec![json!("allow_edits")], json!("end_turn"))
    );
    // The fixture answers the implementation prompt with its fixed reply.
    let reply = agent_text(&exchange.updates);
    assert!(reply.ends_with("fixture response"), "{reply}");
}
