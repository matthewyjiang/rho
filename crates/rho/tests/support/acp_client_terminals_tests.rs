//! Host terminals over a real `rho acp` stdio connection: a host that
//! advertises `terminal` runs `bash` calls in terminals it shows on the
//! call's card, and Rho releases each one after the card's last update.

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace, REPLY_TIMEOUT};

/// The host's answer to one `terminal/*` request, for a command that
/// printed `hi` and exited 0.
fn answer(request: &Value) -> Value {
    match request["method"].as_str() {
        Some("terminal/create") => json!({"terminalId": "term-1"}),
        Some("terminal/wait_for_exit") => json!({"exitCode": 0, "signal": null}),
        Some("terminal/output") => {
            json!({"output": "hi\n", "truncated": false, "exitStatus": {"exitCode": 0}})
        }
        Some("terminal/kill" | "terminal/release") => json!({}),
        _ => panic!("unexpected agent request: {request}"),
    }
}

// Covers: with a host that runs terminals, a `bash` call runs in one: the
// host gets the approved command with Rho's process policy (no stdin, no
// waiting pagers, credentials removed before bash starts), the card
// shows the terminal while running and after finishing, the model gets the
// host's output, and the terminal is released only after the card's final
// update.
// Owner: ACP client terminals, over the real stdio server and shell tool
#[test]
fn bash_runs_in_a_host_terminal_shown_on_its_card() {
    let workspace = Workspace::new();
    let config = workspace.home.path().join("config.toml");
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("permission_mode = \"plan\"", "permission_mode = \"bypass\"");
    std::fs::write(config, settings).unwrap();
    let project = workspace.project.clone();
    let mut server = AcpServer::spawn(workspace.home.path(), &project, "tool-call", &project);
    server
        .request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {"terminal": true}}),
        )
        .0
        .unwrap();
    let created = server
        .request("session/new", json!({"cwd": project, "mcpServers": []}))
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();

    server.next_id += 1;
    let id = server.next_id;
    let call = json!({"id": "run", "name": "bash", "arguments": {"command": "echo hi"}});
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": call.to_string()}]},
    }));
    // What the host saw, in order: requests by method, and card updates by
    // status and content.
    let (mut timeline, mut create, mut reply) = (Vec::new(), Value::Null, String::new());
    loop {
        let message = server
            .messages
            .recv_timeout(REPLY_TIMEOUT)
            .expect("the prompt progresses");
        match message["method"].as_str() {
            Some("session/update") => {
                let update = &message["params"]["update"];
                match update["sessionUpdate"].as_str() {
                    Some("tool_call_update") if update.get("content").is_some() => {
                        timeline.push(json!([update["status"], update["content"]]));
                    }
                    Some("agent_message_chunk") => {
                        reply.push_str(update["content"]["text"].as_str().unwrap_or_default());
                    }
                    _ => {}
                }
            }
            Some(method) => {
                assert_eq!(message["params"]["sessionId"], session_id, "{message}");
                if method == "terminal/create" {
                    create = message["params"].clone();
                }
                timeline.push(json!(method));
                let result = answer(&message);
                server.send(&json!({"jsonrpc": "2.0", "id": message["id"], "result": result}));
            }
            None if message["id"] == id => {
                assert!(message.get("error").is_none(), "{message}");
                break;
            }
            None => panic!("reply to an unknown request: {message}"),
        }
    }

    let terminal = json!([{"type": "terminal", "terminalId": "term-1"}]);
    let args = create["args"].as_array().cloned().unwrap_or_default();
    let shell = args.len().saturating_sub(3);
    let withheld = args[..shell]
        .chunks(2)
        .filter(|pair| pair[0] == "-u")
        .map(|pair| pair[1].clone())
        .collect::<Vec<_>>();
    // The card update and the next request race; only their own orders hold.
    let (requests, updates): (Vec<_>, Vec<_>) =
        timeline.iter().partition(|entry| entry.is_string());
    let position = |entry: &Value| timeline.iter().position(|seen| seen == entry);
    let completed = json!(["completed", terminal]);
    assert_eq!(
        (
            requests,
            updates,
            position(&json!("terminal/release")) > position(&completed),
            json!({
                "command": create["command"],
                "removes_credentials": withheld.contains(&json!("OPENAI_API_KEY")),
                "shell": args[shell..],
                "env": create["env"],
                "cwd": create["cwd"],
                "limited": create["outputByteLimit"].is_u64(),
            }),
            reply.contains("stdout:\nhi"),
        ),
        (
            vec![
                &json!("terminal/create"),
                &json!("terminal/wait_for_exit"),
                &json!("terminal/output"),
                &json!("terminal/release"),
            ],
            vec![&json!(["in_progress", terminal]), &completed],
            true,
            json!({
                "command": "env",
                "removes_credentials": true,
                "shell": ["bash", "-c", "exec </dev/null\necho hi"],
                "env": [{"name": "PAGER", "value": "cat"}, {"name": "GIT_PAGER", "value": "cat"}],
                "cwd": project,
                "limited": true,
            }),
            true,
        )
    );
}
