use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::{agent::CursorTool, cursor_runtime::acp_config::fence};

// Covers: every ACP kind uses its category gate, never Bypass alone; Cursor's
// web search (kind `search`) is gated as network, not as grep. Plan and
// unsupported modes cannot approve even a full tool set. Only Cursor's MCP
// shape (kind `other`, described) maps to MCP; other kinds fail closed.
// Owner: pure Cursor request classification; option selection is generic ACP.
#[test]
fn permission_decision_table() {
    use CursorTool as T;
    use ToolKind as K;
    let cases = [
        ("tool", Some(K::Execute), T::Shell),
        ("web_fetch_0", Some(K::Fetch), T::WebFetch),
        ("tool", Some(K::Edit), T::Edit),
        ("tool", Some(K::Delete), T::Edit),
        ("tool", Some(K::Move), T::ApplyAgentDiff),
        ("tool", Some(K::Read), T::Read),
        ("tool", Some(K::Search), T::Grep),
        // Cursor's web search shares kind `search` with grep; it is network.
        ("web_search_q1", Some(K::Search), T::WebSearch),
        ("tool", Some(K::Other), T::Mcp),
    ];
    for (id, kind, member) in cases {
        let request = RequestPermissionRequest::new(
            "session",
            ToolCallUpdate::new(
                id,
                ToolCallUpdateFields::new().kind(kind).title("server: tool"),
            ),
            vec![PermissionOption::new(
                "allow-once",
                "Allow once",
                PermissionOptionKind::AllowOnce,
            )],
        );
        // A declared tool from another category never approves this request
        // (grep must not approve a web search).
        let other = if member == T::Grep { T::Read } else { T::Grep };
        for (tools, expected) in [
            (vec![member], PermissionDecision::AllowOnce),
            (vec![], PermissionDecision::Reject),
            (vec![other], PermissionDecision::Reject),
        ] {
            assert_eq!(
                decide(
                    &fence(PermissionMode::Bypass, &tools),
                    PermissionMode::Bypass,
                    &request
                ),
                expected,
                "kind {kind:?}, tools {tools:?}"
            );
        }
        let full = fence(PermissionMode::Bypass, T::ALL);
        for mode in [
            PermissionMode::Plan,
            PermissionMode::Auto,
            PermissionMode::AllowEdits,
            PermissionMode::Supervised,
        ] {
            assert_eq!(
                decide(&full, mode, &request),
                PermissionDecision::Reject,
                "mode {mode:?}, kind {kind:?}"
            );
        }
        // A mistakenly passed Bypass mode still cannot widen a Plan fence.
        assert_eq!(
            decide(
                &fence(PermissionMode::Plan, T::ALL),
                PermissionMode::Bypass,
                &request
            ),
            PermissionDecision::Reject
        );
    }
}

// Covers: requests that are not positively MCP (Cursor's undescribed
// fallback, an unnamed server, think, switch_mode, a missing kind) reject even
// when every MCP tool is declared, so the MCP grant cannot approve excluded
// tools.
// Owner: pure Cursor request classification.
#[test]
fn only_described_other_requests_count_as_mcp() {
    let full_mcp = fence(
        PermissionMode::Bypass,
        &[
            CursorTool::Mcp,
            CursorTool::ListMcpResources,
            CursorTool::ReadMcpResource,
        ],
    );
    let future_kind: ToolKind = serde_json::from_value(json!("future_kind")).unwrap();
    for (kind, title, expected) in [
        (
            Some(ToolKind::Other),
            "server: tool",
            PermissionDecision::AllowOnce,
        ),
        (
            Some(future_kind),
            "server: tool",
            PermissionDecision::AllowOnce,
        ),
        (
            Some(ToolKind::Other),
            "Unknown operation",
            PermissionDecision::Reject,
        ),
        (Some(ToolKind::Other), ": tool", PermissionDecision::Reject),
        (
            Some(ToolKind::Think),
            "server: tool",
            PermissionDecision::Reject,
        ),
        (
            Some(ToolKind::SwitchMode),
            "server: tool",
            PermissionDecision::Reject,
        ),
        (None, "server: tool", PermissionDecision::Reject),
    ] {
        let request = RequestPermissionRequest::new(
            "session",
            ToolCallUpdate::new("tool", ToolCallUpdateFields::new().kind(kind).title(title)),
            vec![PermissionOption::new(
                "allow-once",
                "Allow once",
                PermissionOptionKind::AllowOnce,
            )],
        );
        assert_eq!(
            decide(&full_mcp, PermissionMode::Bypass, &request),
            expected,
            "kind {kind:?}, title {title}"
        );
    }
}

// Covers: ask-question's permission fallback must skip, not accidentally choose
// an answer merely because MCP/other is allowed by the declared tools.
// Owner: pure Cursor permission fallback policy.
#[test]
fn ask_question_permission_fallback_is_skipped() {
    let request = RequestPermissionRequest::new(
        "session",
        ToolCallUpdate::new(
            "question",
            ToolCallUpdateFields::new().kind(ToolKind::Other),
        ),
        vec![
            PermissionOption::new("answer-a", "A", PermissionOptionKind::AllowOnce),
            PermissionOption::new("answer-b", "B", PermissionOptionKind::AllowOnce),
            PermissionOption::new(
                "__ask_question_skip__",
                "Skip",
                PermissionOptionKind::RejectOnce,
            ),
        ],
    );
    for mode in [PermissionMode::Bypass, PermissionMode::Plan] {
        assert_eq!(
            decide(&fence(mode, CursorTool::ALL), mode, &request),
            PermissionDecision::Reject
        );
    }
}
