use pretty_assertions::assert_eq;
use rho_providers::model::Message;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::*;
use crate::session::Session;

// Covers: recall resolves a persisted result by id from the current session,
// pages it by character window, and fails clearly for an unknown id or a
// session without a transcript.
// Owner: current-session recall storage lookup.
#[test]
fn recalls_persisted_tool_result_by_id() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let result = ToolResult {
        id: "call_0".into(),
        ok: false,
        content: "héllo world".into(),
    };
    session
        .append_message(&Message::ToolResult(result.clone()))
        .unwrap();
    let id = crate::compaction::recall_id(&result);
    let recall = |current: &str, recall_id: &str, window| {
        super::recall(
            root.path(),
            cwd.path(),
            current,
            recall_id,
            window,
            &CancellationToken::new(),
        )
        .map(|output| serde_json::from_str::<Value>(&output).unwrap())
        .map_err(|error| error.to_string())
    };

    assert_eq!(
        recall(session.id(), &id, (1, 4)),
        Ok(json!({
            "note": UNTRUSTED,
            "recall_id": id,
            "tool_call_id": "call_0",
            "ok": false,
            "text": "éllo",
            "start": 1,
            "end": 5,
            "total_chars": 11,
            "next_start": 5,
        }))
    );
    assert_eq!(
        recall(session.id(), "rmissing", (0, 10)),
        Err(
            "unknown recall_id 'rmissing': no tool result with this id in the current session"
                .into()
        )
    );
    assert!(recall("no-such-session", &id, (0, 10))
        .unwrap_err()
        .starts_with("current session transcript is not available for recall"));
}
