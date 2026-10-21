//! Session history lifecycle over the real stdio server: deleting, listing
//! with workspace roots, and resuming saved sessions.

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{read_the_extra_file, AcpServer, Workspace};

/// Starts a server and completes `initialize`, returning its result.
fn initialized(workspace: &Workspace) -> (AcpServer, Value) {
    let mut server = workspace.server();
    let initialized = server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    (server, initialized)
}

fn new_session(server: &mut AcpServer, workspace: &Workspace, roots: Value) -> String {
    let created = server
        .request(
            "session/new",
            json!({"cwd": workspace.project, "mcpServers": [], "additionalDirectories": roots}),
        )
        .0
        .unwrap();
    created["sessionId"].as_str().unwrap().to_owned()
}

/// The ids `session/list` reports for the workspace.
fn listed_ids(server: &mut AcpServer, workspace: &Workspace) -> Vec<String> {
    let listed = server
        .request("session/list", json!({"cwd": workspace.project}))
        .0
        .unwrap();
    listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|info| info["sessionId"].as_str().unwrap().to_owned())
        .collect()
}

/// Where a session stands when the host deletes it.
#[derive(Clone, Copy, Debug)]
enum DeleteTarget {
    NeverCreated,
    LiveUnsaved,
    LiveSaved,
    AlreadyDeleted,
}

// Covers: session/delete must succeed whether or not the session is saved,
// live on this connection, both, or already deleted, and always end with no
// live session and nothing saved. A live session with nothing saved yet used
// to fail resource_not_found and stay open.
// Owner: ACP session lifecycle, over the real stdio server
#[test]
fn delete_converges_whatever_the_session_state() {
    let workspace = Workspace::new();
    let (mut server, _) = initialized(&workspace);
    for target in [
        DeleteTarget::NeverCreated,
        DeleteTarget::LiveUnsaved,
        DeleteTarget::LiveSaved,
        DeleteTarget::AlreadyDeleted,
    ] {
        let session_id = match target {
            DeleteTarget::NeverCreated => "0b6f7d0e-5b2c-4c8e-9a51-3f1d2a7c9e44".to_owned(),
            DeleteTarget::LiveUnsaved => new_session(&mut server, &workspace, json!([])),
            DeleteTarget::LiveSaved | DeleteTarget::AlreadyDeleted => {
                let session_id = new_session(&mut server, &workspace, json!([]));
                server.prompt(&session_id, "read the shared notes");
                session_id
            }
        };
        if matches!(target, DeleteTarget::AlreadyDeleted) {
            let (deleted, _) = server.request("session/delete", json!({"sessionId": session_id}));
            deleted.unwrap();
        }

        let (deleted, _) = server.request("session/delete", json!({"sessionId": session_id}));
        assert_eq!(deleted, Ok(json!({})), "{target:?}");
        let (prompted, _) = server.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "hi"}]}),
        );
        assert_eq!(prompted.unwrap_err()["code"], -32002, "{target:?}");
        assert!(
            !listed_ids(&mut server, &workspace).contains(&session_id),
            "{target:?}"
        );
    }
}

// Covers: session/list must report each session's saved roots as
// additionalDirectories, leaving out roots deleted since, because
// session/load restores only the roots its request names: a host loading
// from history sends back what it listed, and a gone root would fail the load.
// Owner: ACP session/list, over the real stdio server
#[test]
fn listed_roots_round_trip_through_load() {
    let workspace = Workspace::new();
    let gone = workspace.home.path().canonicalize().unwrap().join("gone");
    std::fs::create_dir_all(&gone).unwrap();
    let (mut server, _) = initialized(&workspace);
    let session_id = new_session(&mut server, &workspace, json!([workspace.extra, gone]));
    server.prompt(&session_id, "read the shared notes");
    drop(server);
    std::fs::remove_dir(&gone).unwrap();

    let (mut server, _) = initialized(&workspace);
    let listed = server
        .request("session/list", json!({"cwd": workspace.project}))
        .0
        .unwrap();
    let roots = listed["sessions"][0]["additionalDirectories"].clone();
    assert_eq!(
        (&listed["sessions"][0]["sessionId"], &roots),
        (&json!(session_id), &json!([workspace.extra]))
    );
    let (loaded, _) = server.request(
        "session/load",
        json!({"sessionId": session_id, "cwd": workspace.project, "mcpServers": [], "additionalDirectories": roots}),
    );
    loaded.unwrap();
    let reply = server.prompt(&session_id, "read the shared notes");
    assert!(read_the_extra_file(&reply), "{reply}");
}

// Covers: session/resume (advertised as sessionCapabilities.resume) must
// restore a saved session with the roots it names, answer with modes and
// config options, and send nothing before its response: no history replay,
// with the command list following the response as on session/load. The
// resumed session must continue the saved conversation, not start over.
// Owner: ACP session lifecycle, over the real stdio server
#[test]
fn resume_restores_the_saved_session_without_replay() {
    let workspace = Workspace::new();
    let (mut server, _) = initialized(&workspace);
    let session_id = new_session(&mut server, &workspace, json!([workspace.extra]));
    server.prompt(&session_id, "read the shared notes");
    drop(server);

    let (mut server, initialized) = initialized(&workspace);
    assert_eq!(
        initialized["agentCapabilities"]["sessionCapabilities"]["resume"],
        json!({})
    );
    let (resumed, before_response) = server.request(
        "session/resume",
        json!({"sessionId": session_id, "cwd": workspace.project, "mcpServers": [], "additionalDirectories": [workspace.extra]}),
    );
    let resumed = resumed.unwrap();
    assert_eq!(
        (
            before_response,
            &resumed["modes"]["currentModeId"],
            resumed["configOptions"].is_array(),
        ),
        (vec![], &json!("plan"), true)
    );
    let (prompted, updates) = server.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "read the shared notes"}]}),
    );
    prompted.unwrap();
    assert_eq!(updates[0]["sessionUpdate"], "available_commands_update");
    let reply: String = updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update["content"]["text"].as_str())
        .collect();
    assert!(read_the_extra_file(&reply), "{reply}");

    let (loaded, replay) = server.request(
        "session/load",
        json!({"sessionId": session_id, "cwd": workspace.project, "mcpServers": []}),
    );
    loaded.unwrap();
    let replayed_prompts = replay
        .iter()
        .filter(|update| update["sessionUpdate"] == "user_message_chunk")
        .count();
    assert_eq!(replayed_prompts, 2, "{replay:#?}");
}
