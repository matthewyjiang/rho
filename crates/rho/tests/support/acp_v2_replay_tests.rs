//! ACP v2 `session/resume`: the replay ends with `replay_complete`, live
//! updates and requests wait past it, the session's current state follows,
//! and a lost daemon link is reported as the `unknown` state.

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{stop_daemon, AcpServer, Workspace};

/// A v2 `initialize`.
fn initialize_v2(server: &mut AcpServer) {
    server
        .request(
            "initialize",
            json!({
                "protocolVersion": 2,
                "info": {"name": "replay-tests", "version": "1.0.0"},
                "capabilities": {},
            }),
        )
        .0
        .unwrap();
}

fn new_session(server: &mut AcpServer, workspace: &Workspace) -> String {
    let created = server
        .request("session/new", json!({"cwd": workspace.project}))
        .0
        .unwrap();
    created["sessionId"].as_str().unwrap().to_owned()
}

/// Sends `method` and returns the reply and every message the agent sent
/// before it, notifications and requests alike, in order.
fn exchange(
    server: &mut AcpServer,
    method: &str,
    params: Value,
) -> (Result<Value, Value>, Vec<Value>) {
    server.next_id += 1;
    let id = server.next_id;
    server.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    let mut before = Vec::new();
    loop {
        let message = server.recv(&format!("reply to {method}"));
        if message["id"] == id && message.get("method").is_none() {
            let reply = match message.get("error") {
                Some(error) => Err(error.clone()),
                None => Ok(message["result"].clone()),
            };
            return (reply, before);
        }
        before.push(message);
    }
}

/// What a message is, for comparing traffic: an update's `sessionUpdate`,
/// with a state's `state`, or a request's method.
fn kind(message: &Value) -> String {
    match message["method"].as_str() {
        Some("session/update") => {
            let update = &message["params"]["update"];
            let kind = update["sessionUpdate"].as_str().unwrap_or_default();
            match update["state"].as_str() {
                Some(state) if kind == "state_update" => format!("state:{state}"),
                _ => kind.to_owned(),
            }
        }
        Some(method) => method.to_owned(),
        None => "reply".to_owned(),
    }
}

fn kinds(messages: &[Value]) -> Vec<String> {
    messages.iter().map(kind).collect()
}

/// The messages before and after the one `replay_complete`, which must
/// arrive exactly once.
fn split_at_marker(messages: &[Value]) -> (&[Value], &[Value]) {
    let markers: Vec<_> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| kind(message) == "replay_complete")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(markers.len(), 1, "{:#?}", kinds(messages));
    (&messages[..markers[0]], &messages[markers[0] + 1..])
}

/// Replayed history: conversation updates only, each marked `isReplay` on
/// the notification, never in the entity's `_meta`, which v2 keeps.
fn assert_history(replayed: &[Value]) {
    for message in replayed {
        let kind = kind(message);
        assert!(
            matches!(
                kind.as_str(),
                "user_message"
                    | "user_message_chunk"
                    | "agent_message"
                    | "agent_message_chunk"
                    | "agent_thought"
                    | "agent_thought_chunk"
                    | "tool_call_update"
            ),
            "not history before replay_complete: {message:#}"
        );
        assert_eq!(
            (
                &message["params"]["_meta"],
                message["params"]["update"].get("_meta")
            ),
            (&json!({"isReplay": true}), None),
            "{message:#}"
        );
    }
}

/// The updates of `messages`, which carry no notification `_meta`: they
/// are live, not replayed.
fn live_updates(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            assert_eq!(message["params"].get("_meta"), None, "{message:#}");
            message["params"]["update"].clone()
        })
        .collect()
}

/// `rho acp` in bypass mode on the tool-call fixture, with a cost on each
/// response and a generated title.
fn titled_server(workspace: &Workspace) -> AcpServer {
    let config = workspace.home.config_path.clone();
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("permission_mode = \"plan\"", "permission_mode = \"bypass\"");
    std::fs::write(config, settings).unwrap();
    let mut command = AcpServer::command(&workspace.home, "tool-call", &workspace.project);
    command
        .env("RHO_AUTOMATION_TEST_COST_USD_MICROS", "1250")
        // The title agent's provider skips the automation fixture; the TUI
        // fixture answers its request instead.
        .env("RHO_TUI_TEST_MODE", "matrix");
    AcpServer::from_command(&mut command)
}

// Covers: a v2 resume with `replayFrom` sends only history, then exactly
// one `replay_complete`, also for an empty replay, then the current state,
// title and usage, which history leaves out, before the response; a resume
// without `replayFrom` sends no marker and no history but the same current
// state. Replayed updates are marked per notification, so the `isReplay`
// marker never sticks to the entities v2 keeps.
// Owner: ACP v2 boundary, over the real stdio server
#[test]
fn v2_resume_ends_the_replay_with_replay_complete_then_current_state() {
    let workspace = Workspace::new();
    let mut server = titled_server(&workspace);
    initialize_v2(&mut server);
    let session_id = new_session(&mut server, &workspace);
    let replay =
        json!({"sessionId": session_id, "cwd": workspace.project, "replayFrom": {"type": "start"}});
    let resume = json!({"sessionId": session_id, "cwd": workspace.project});

    let (empty, before) = exchange(&mut server, "session/resume", replay.clone());
    empty.unwrap();
    let (replayed, current) = split_at_marker(&before);
    assert_eq!(
        (replayed.len(), live_updates(current)),
        (
            0,
            vec![
                json!({"sessionUpdate": "state_update", "state": "idle", "background": []}),
                json!({"sessionUpdate": "session_info_update"}),
            ]
        )
    );

    let (reply, updates) = server.prompt_v2(&session_id, "hello");
    reply.unwrap();
    // The title is generated beside the turn, and may land after it.
    let titled = |update: &Value| update["sessionUpdate"] == "session_info_update";
    if !updates.iter().any(titled) {
        while !titled(&server.next_notification()["update"]) {}
    }
    let usage = updates
        .iter()
        .rfind(|update| update["sessionUpdate"] == "usage_update")
        .expect("the turn reports its usage")
        .clone();

    let (resumed, before) = exchange(&mut server, "session/resume", replay);
    resumed.unwrap();
    let (replayed, current) = split_at_marker(&before);
    assert_history(replayed);
    assert_eq!(
        kinds(replayed),
        [
            "user_message",
            "user_message_chunk",
            "agent_message",
            "agent_message_chunk"
        ]
    );
    let titled_current = vec![
        json!({"sessionUpdate": "state_update", "state": "idle", "background": []}),
        json!({"sessionUpdate": "session_info_update", "title": "Fixture session title"}),
        usage,
    ];
    assert_eq!(live_updates(current), titled_current);

    let (resumed, before) = exchange(&mut server, "session/resume", resume);
    resumed.unwrap();
    assert_eq!(live_updates(&before), titled_current);
}

// Covers: a v2 host that resumes with replay while another client's turn
// waits on a permission gets that turn's updates so far as history, then
// `replay_complete`, `requires_action` and the response, and only then the
// pending permission request and the turn's later updates, which go on
// live until its end.
// Owner: ACP v2 boundary and session pump, over the real stdio server
#[test]
fn v2_resume_during_another_clients_turn_cuts_at_replay_complete() {
    let workspace = Workspace::new();
    let mut first = AcpServer::spawn(&workspace.home, "tool-call", &workspace.project);
    initialize_v2(&mut first);
    let session_id = new_session(&mut first, &workspace);
    // The session opens in plan mode, where leaving it asks the host.
    let call = json!({
        "id": "plan", "name": "exit_plan_mode",
        "arguments": {"plan": "1. Change the thing."},
    });
    first.next_id += 1;
    first.send(&json!({
        "jsonrpc": "2.0", "id": first.next_id, "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": call.to_string()}]},
    }));
    while first.recv("the first host's permission request")["method"]
        != "session/request_permission"
    {}

    let mut second = AcpServer::spawn(&workspace.home, "tool-call", &workspace.project);
    initialize_v2(&mut second);
    let (resumed, before) = exchange(
        &mut second,
        "session/resume",
        json!({"sessionId": session_id, "cwd": workspace.project, "replayFrom": {"type": "start"}}),
    );
    resumed.unwrap();
    let (replayed, current) = split_at_marker(&before);
    assert_history(replayed);
    assert_eq!(
        (kinds(replayed), kinds(current)),
        (
            vec![
                "user_message".to_owned(),
                "user_message_chunk".to_owned(),
                "tool_call_update".to_owned()
            ],
            vec![
                "state:requires_action".to_owned(),
                "session_info_update".to_owned(),
                "usage_update".to_owned()
            ],
        )
    );

    let asked = second.recv("the pending permission request");
    assert_eq!(kind(&asked), "session/request_permission", "{asked:#}");
    second.send(&json!({
        "jsonrpc": "2.0", "id": asked["id"],
        "result": {"outcome": {"outcome": "selected", "optionId": "keep_planning"}},
    }));
    let mut live = Vec::new();
    loop {
        let message = second.recv("the turn's live updates");
        let idle = kind(&message) == "state:idle";
        live.push(message);
        if idle {
            break;
        }
    }
    let states: Vec<_> = live_updates(&live)
        .into_iter()
        .filter(|update| update["sessionUpdate"] == "state_update")
        .collect();
    assert_eq!(
        states,
        [
            json!({"sessionUpdate": "state_update", "state": "running", "background": []}),
            json!({
                "sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn",
                "background": [],
            }),
        ]
    );
}

// Covers: a v2 host that resumes, with replay, the session whose prompt it
// is running gets the same cut as any other host, and its prompts run on:
// the permission it was asked stays the one to answer, the turn ends
// `end_turn`, not cancelled, and a prompt held behind it is inserted next.
// Owner: ACP sessions (in-place resync) and session pump, over the real
// stdio server
#[test]
fn v2_resume_of_its_own_running_prompt_keeps_the_prompt() {
    let workspace = Workspace::new();
    let mut server = AcpServer::spawn(&workspace.home, "tool-call", &workspace.project);
    initialize_v2(&mut server);
    let session_id = new_session(&mut server, &workspace);
    // The session opens in plan mode, where leaving it asks the host.
    let call = json!({
        "id": "plan", "name": "exit_plan_mode",
        "arguments": {"plan": "1. Change the thing."},
    });
    server.next_id += 1;
    server.send(&json!({
        "jsonrpc": "2.0", "id": server.next_id, "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": call.to_string()}]},
    }));
    let asked = loop {
        let message = server.recv("the prompt's permission request");
        if message["method"] == "session/request_permission" {
            break message;
        }
    };
    server.next_id += 1;
    let held = server.next_id;
    server.send(&json!({
        "jsonrpc": "2.0", "id": held, "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": "hello"}]},
    }));

    let (resumed, before) = exchange(
        &mut server,
        "session/resume",
        json!({"sessionId": session_id, "cwd": workspace.project, "replayFrom": {"type": "start"}}),
    );
    resumed.unwrap();
    let (replayed, current) = split_at_marker(&before);
    assert_history(replayed);
    let resynced = (kinds(replayed), kinds(current));

    server.send(&json!({
        "jsonrpc": "2.0", "id": asked["id"],
        "result": {"outcome": {"outcome": "selected", "optionId": "keep_planning"}},
    }));
    // Both turns' traffic, through the held prompt's end.
    let mut live = Vec::new();
    let mut idles = 0;
    while idles < 2 {
        let message = server.recv("the turns' live traffic");
        idles += usize::from(kind(&message) == "state:idle");
        live.push(message);
    }
    let held_reply = live
        .iter()
        .position(|message| message["id"] == held && message.get("method").is_none())
        .expect("the held prompt is answered");
    let first_idle = live
        .iter()
        .position(|message| kind(message) == "state:idle")
        .unwrap();
    assert!(
        !kinds(&live).contains(&"session/request_permission".to_owned()),
        "the pending permission was asked again: {:#?}",
        kinds(&live)
    );
    assert_eq!(
        resynced,
        (
            vec![
                "user_message".to_owned(),
                "user_message_chunk".to_owned(),
                "tool_call_update".to_owned()
            ],
            vec![
                "state:requires_action".to_owned(),
                "session_info_update".to_owned(),
                "usage_update".to_owned()
            ],
        )
    );
    let end_turn = json!({
        "sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn",
        "background": [],
    });
    let idles: Vec<_> = live
        .iter()
        .filter(|message| kind(message) == "state:idle")
        .map(|message| message["params"]["update"].clone())
        .collect();
    assert_eq!(
        (
            idles,
            held_reply > first_idle,
            live[held_reply]["result"]["messageId"].is_string()
        ),
        (vec![end_turn.clone(), end_turn], true, true)
    );
}

// Covers: a v2 host whose `rho acp` loses the daemon must hear `unknown`,
// with no background list, not a made-up end. Once a request attaches
// again, the host hears the session's state before that request's answer;
// a prompt's own `running` reports it instead, since an `idle` first would
// end T3 Code's pending prompt.
// Owner: ACP session pump and v2 port, over the real stdio server
#[test]
fn v2_reports_unknown_when_the_daemon_link_is_lost() {
    let workspace = Workspace::new();
    let mut server = AcpServer::spawn(&workspace.home, "tool-call", &workspace.project);
    initialize_v2(&mut server);
    let session_id = new_session(&mut server, &workspace);
    // The daemon saves a session once it is prompted, so the next one can
    // reopen it.
    server.prompt_v2(&session_id, "hello").0.unwrap();
    let unknown = json!({
        "sessionId": session_id,
        "update": {"sessionUpdate": "state_update", "state": "unknown"},
    });

    stop_daemon(&workspace.home);
    let lost = server.next_notification();
    let (reply, prompted) = server.prompt_v2(&session_id, "hello");
    reply.unwrap();
    let prompted: Vec<_> = prompted.iter().map(kind_of_update).collect();

    stop_daemon(&workspace.home);
    let lost_again = server.next_notification();
    let (configured, before) = exchange(
        &mut server,
        "session/set_config_option",
        json!({"sessionId": session_id, "configId": "mode", "type": "id", "value": "plan"}),
    );
    configured.unwrap();

    assert_eq!(
        (
            lost,
            lost_again,
            &prompted[..2],
            &live_updates(&before)[..1]
        ),
        (
            unknown.clone(),
            unknown,
            &["user_message".to_owned(), "state:running".to_owned()][..],
            &[json!({"sessionUpdate": "state_update", "state": "idle", "background": []})][..],
        )
    );
}

/// An update's `sessionUpdate`, with a state's `state`.
fn kind_of_update(update: &Value) -> String {
    kind(&json!({"method": "session/update", "params": {"update": update}}))
}
