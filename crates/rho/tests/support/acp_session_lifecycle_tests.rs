//! Session history lifecycle over the real stdio server: deleting, listing
//! with workspace roots, and resuming saved sessions.

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace};

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
