//! A v2 host's `systemPrompt`: advertised, applied to the model's system
//! prompt on `session/new` and `session/resume`, kept by a resume that
//! omits it, and refused with `-32602` when Rho can't apply it.

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace};

fn system_prompt(mode: &str, text: &str) -> Value {
    json!({"mode": mode, "content": [{"type": "text", "text": text}]})
}

/// `rho acp` with the fixture answering each turn with the model request it
/// got, initialized for v2, and the `initialize` result.
fn inspecting_v2(workspace: &Workspace) -> (AcpServer, Value) {
    let mut server = AcpServer::spawn(&workspace.home, "inspect", &workspace.project);
    let initialized = server
        .request(
            "initialize",
            json!({
                "protocolVersion": 2,
                "info": {"name": "system-prompt-test", "version": "1.0.0"},
                "capabilities": {},
            }),
        )
        .0
        .unwrap();
    (server, initialized)
}

fn new_session(server: &mut AcpServer, cwd: &std::path::Path, prompt: Option<Value>) -> String {
    let mut params = json!({"cwd": cwd});
    if let Some(prompt) = prompt {
        params["systemPrompt"] = prompt;
    }
    let (created, _) = server.request("session/new", params);
    created.unwrap()["sessionId"].as_str().unwrap().to_owned()
}

/// The system prompt of the model request a turn in `session` sent.
fn model_system_prompt(server: &mut AcpServer, session: &str) -> String {
    let (reply, updates) = server.prompt_v2(session, "hello");
    reply.unwrap();
    let answer: String = updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update["content"]["text"].as_str())
        .collect();
    let request: Value = serde_json::from_str(&answer).expect("the fixture answers with JSON");
    request["messages"][0]["System"]
        .as_str()
        .expect("the request has a system prompt")
        .to_owned()
}

// Covers: `session.systemPrompt` is advertised with `override`; `append`
// adds the host's text after Rho's whole prompt; `override` replaces Rho's
// default instructions but keeps the workspace's AGENTS.md; a resume with
// another prompt applies it to a session the daemon has loaded, and a
// resume that omits it keeps the last one.
// Owner: ACP v2 session setup and daemon prompt assembly, over the real
// stdio server
#[test]
fn v2_applies_a_hosts_system_prompt_on_new_and_resume() {
    let workspace = Workspace::new();
    std::fs::write(workspace.project.join("AGENTS.md"), "Workspace rule 7.").unwrap();
    let (mut server, initialized) = inspecting_v2(&workspace);
    assert_eq!(
        initialized["capabilities"]["session"]["systemPrompt"],
        json!({"mode": {"override": {}}})
    );
    let cwd = workspace.project.clone();
    let plain = new_session(&mut server, &cwd, None);
    let default = model_system_prompt(&mut server, &plain);

    let appended = new_session(
        &mut server,
        &cwd,
        Some(system_prompt("append", "Review for security.")),
    );
    assert_eq!(
        model_system_prompt(&mut server, &appended),
        format!("{default}\n\n# Host instructions\n\nReview for security.\n")
    );

    let overridden = new_session(
        &mut server,
        &cwd,
        Some(system_prompt("override", "You are a pirate.")),
    );
    let first_line = default.lines().next().unwrap();
    let summary = |system: &str| {
        (
            system.starts_with("You are a pirate.\n"),
            system.contains(first_line),
            system.contains("Workspace rule 7."),
        )
    };
    let overriding = model_system_prompt(&mut server, &overridden);
    assert_eq!(summary(&overriding), (true, false, true), "{overriding}");

    let resume = |server: &mut AcpServer, prompt: Option<Value>| {
        let mut params = json!({"sessionId": appended, "cwd": cwd});
        if let Some(prompt) = prompt {
            params["systemPrompt"] = prompt;
        }
        server.request("session/resume", params).0.unwrap();
    };
    resume(
        &mut server,
        Some(system_prompt("override", "You are a pirate.")),
    );
    let replaced = model_system_prompt(&mut server, &appended);
    resume(&mut server, None);
    let kept = model_system_prompt(&mut server, &appended);
    assert_eq!(summary(&replaced), (true, false, true), "{replaced}");
    assert_eq!(kept, replaced);
}

// Covers: a systemPrompt Rho can't apply gets `-32602` on `session/new`
// and `session/resume`: an unadvertised mode, a non-text block, no
// content, and no text.
// Owner: ACP v2 session setup validation, over the real stdio server
#[test]
fn v2_refuses_a_system_prompt_it_cannot_apply() {
    let workspace = Workspace::new();
    let (mut server, _) = inspecting_v2(&workspace);
    let cwd = workspace.project.clone();
    let session = new_session(&mut server, &cwd, None);
    let image = json!({"type": "image", "data": "AAAA", "mimeType": "image/png"});
    let cases = [
        ("a custom mode", system_prompt("_custom", "hi")),
        (
            "an image block",
            json!({"mode": "append", "content": [image]}),
        ),
        ("no content", json!({"mode": "append", "content": []})),
        ("blank text", system_prompt("override", "  ")),
    ];
    let mut refused = Vec::new();
    for (case, prompt) in cases {
        let (created, _) =
            server.request("session/new", json!({"cwd": cwd, "systemPrompt": prompt}));
        let (resumed, _) = server.request(
            "session/resume",
            json!({"sessionId": session, "cwd": cwd, "systemPrompt": prompt}),
        );
        refused.push((
            case,
            created.map_err(|error| error["code"].clone()),
            resumed.map_err(|error| error["code"].clone()),
        ));
    }

    assert_eq!(
        refused,
        [
            ("a custom mode", Err(json!(-32602)), Err(json!(-32602))),
            ("an image block", Err(json!(-32602)), Err(json!(-32602))),
            ("no content", Err(json!(-32602)), Err(json!(-32602))),
            ("blank text", Err(json!(-32602)), Err(json!(-32602))),
        ]
    );
}
