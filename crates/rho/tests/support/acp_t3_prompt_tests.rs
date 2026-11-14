//! T3 Code's prompt wrapping (`rho acp --t3code`) over the real stdio
//! server and the daemon behind it: the model gets it, the saved user
//! message does not.

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace};

const INSTRUCTIONS: &str =
    "<t3_code_instructions>\n## T3 Code interaction mode: Default\n</t3_code_instructions>";
const RUNTIME: &str =
    "<runtime_info>In case you're asked: you are running in T3 Code.</runtime_info>";
const LINKING: &str = "<pull_request_linking>\nLink each PR you create.\n</pull_request_linking>";
const TYPED: &str = "fix the login bug";

/// A `rho acp --t3code` server whose model answers with the request it got.
fn t3_server(workspace: &Workspace) -> AcpServer {
    let mut command = AcpServer::command(&workspace.home, "inspect", &workspace.project);
    let mut server = AcpServer::from_command(command.arg("--t3code"));
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    server
}

/// The text blocks of the last user message in an inspected model request.
/// The fixture provider strips the time the daemon stamps on the first.
fn last_user_texts(inspection: &Value) -> Vec<&str> {
    let user = inspection["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find_map(|message| message.get("User"))
        .unwrap();
    user.as_array()
        .unwrap()
        .iter()
        .map(|block| block["Text"].as_str().unwrap())
        .collect()
}

// Covers: a T3 prompt's instructions, runtime info, and PR-linking blocks
// reach the model, while the saved user message, which `session/list`
// labels the session with and `session/load` replays, holds only what the
// person typed
// Owner: ACP `--t3code` prompts and daemon prompt context, over the real
// stdio server
#[test]
fn t3_wrapping_reaches_the_model_but_not_the_saved_message() {
    let workspace = Workspace::new();
    let mut server = t3_server(&workspace);
    let created = server
        .request(
            "session/new",
            json!({"cwd": workspace.project, "mcpServers": []}),
        )
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();
    let prompt = json!([
        {"type": "text", "text": format!("{INSTRUCTIONS}\n\n<user_request>\n{TYPED}\n</user_request>")},
        {"type": "text", "text": format!("{RUNTIME}\n\n{LINKING}")},
    ]);
    let (replied, updates) = server.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": prompt}),
    );
    replied.unwrap();
    let reply: String = updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update["content"]["text"].as_str())
        .collect();
    let inspection: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(
        last_user_texts(&inspection),
        [INSTRUCTIONS, RUNTIME, LINKING, TYPED]
    );

    let listed = server
        .request("session/list", json!({"cwd": workspace.project}))
        .0
        .unwrap();
    assert_eq!(listed["sessions"][0]["title"], TYPED);
    drop(server);

    let mut server = t3_server(&workspace);
    let (loaded, replayed) = server.request(
        "session/load",
        json!({"sessionId": session_id, "cwd": workspace.project, "mcpServers": []}),
    );
    loaded.unwrap();
    let shown: Vec<&Value> = replayed
        .iter()
        .filter(|update| update["sessionUpdate"] == "user_message_chunk")
        .map(|update| &update["content"])
        .collect();
    assert_eq!(shown, [&json!({"type": "text", "text": TYPED})]);
}
