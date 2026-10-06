use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn allowed(mode: PermissionMode, tools: &[AntigravityTool]) -> AllowedTools {
    map_permission_mode(mode, tools).expect("test fence is valid")
}

// Covers: Plan keeps only read-only built-ins (and needs one), Bypass keeps
// the declared list, and modes that would need a human approver are refused.
// The kept list is exactly what `session/new` allowlists.
// Owner: Antigravity fence derivation; bind and plan host only call it.
#[test]
fn permission_mode_narrows_the_allowlist() {
    use AntigravityTool as T;
    let declared = [T::ViewFile, T::RunCommand, T::SearchWeb];
    let cases = [
        (
            PermissionMode::Plan,
            &declared[..],
            Ok(json!({"agy": {"enabledTools": ["view_file"]}})),
        ),
        (
            PermissionMode::Bypass,
            &declared[..],
            Ok(json!({"agy": {"enabledTools": ["view_file", "run_command", "search_web"]}})),
        ),
        (
            PermissionMode::Plan,
            &[T::CreateFile][..],
            Err(AntigravityFenceError::NoReadOnlyToolsInPlan),
        ),
        (
            PermissionMode::Bypass,
            &[][..],
            Err(AntigravityFenceError::NoToolsAllowed),
        ),
        (
            PermissionMode::Auto,
            &declared[..],
            Err(AntigravityFenceError::ApprovalUnsupported(
                PermissionMode::Auto,
            )),
        ),
        (
            PermissionMode::AllowEdits,
            &declared[..],
            Err(AntigravityFenceError::ApprovalUnsupported(
                PermissionMode::AllowEdits,
            )),
        ),
        (
            PermissionMode::Supervised,
            &declared[..],
            Err(AntigravityFenceError::ApprovalUnsupported(
                PermissionMode::Supervised,
            )),
        ),
    ];
    for (mode, tools, expected) in cases {
        let actual = map_permission_mode(mode, tools)
            .map(|allowed| serde_json::Value::Object(allowed.session_meta()));
        assert_eq!(actual, expected, "{mode} {tools:?}");
    }
}

// Covers: each permission kind agy_acp_server 1.3.0 sends approves only when
// a built-in of that category is declared, never on Bypass alone; Plan and
// unsupported modes approve nothing; unclassified kinds fail closed.
// Owner: Antigravity permission answers; option selection is generic ACP.
#[test]
fn permission_decision_table() {
    use AntigravityTool as T;
    use ToolKind as K;
    let request = |kind: Option<ToolKind>| {
        RequestPermissionRequest::new(
            "session",
            ToolCallUpdate::new("call", ToolCallUpdateFields::new().kind(kind)),
            vec![PermissionOption::new(
                "allow",
                "Allow",
                PermissionOptionKind::AllowOnce,
            )],
        )
    };
    let cases = [
        (K::Read, T::ViewFile),
        (K::Edit, T::CreateFile),
        (K::Edit, T::EditFile),
        (K::Delete, T::EditFile),
        (K::Move, T::EditFile),
        (K::Execute, T::RunCommand),
        (K::Search, T::SearchWeb),
        (K::Fetch, T::ReadUrlContent),
    ];
    for (kind, member) in cases {
        let request = request(Some(kind));
        let other = if member == T::RunCommand {
            T::ViewFile
        } else {
            T::RunCommand
        };
        for (tools, expected) in [
            (member, PermissionDecision::AllowOnce),
            (other, PermissionDecision::Reject),
        ] {
            assert_eq!(
                decide(
                    &allowed(PermissionMode::Bypass, &[tools]),
                    PermissionMode::Bypass,
                    &request
                ),
                expected,
                "kind {kind:?}, tools {tools:?}"
            );
        }
        let full = allowed(PermissionMode::Bypass, T::ALL);
        for mode in [
            PermissionMode::Plan,
            PermissionMode::Auto,
            PermissionMode::AllowEdits,
            PermissionMode::Supervised,
        ] {
            assert_eq!(
                decide(&full, mode, &request),
                PermissionDecision::Reject,
                "{mode} {kind:?}"
            );
        }
    }
    let full = allowed(PermissionMode::Bypass, T::ALL);
    for kind in [Some(K::Other), Some(K::Think), Some(K::SwitchMode), None] {
        assert_eq!(
            decide(&full, PermissionMode::Bypass, &request(kind)),
            PermissionDecision::Reject,
            "{kind:?}"
        );
    }
}
