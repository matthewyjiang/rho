use agent_client_protocol::v1::schema::{
    PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
};
use pretty_assertions::assert_eq;

use super::*;

fn fence(mode: PermissionMode, tools: &[AntigravityTool]) -> AntigravityFence {
    AntigravityFence::new(mode, tools).expect("test fence is valid")
}

// Covers: Plan keeps only read-only built-ins (and needs one) in agy mode
// `plan`, Bypass keeps the declared list in mode `default`, and modes that
// would need a human approver are refused.
// Owner: Antigravity fence derivation; bind and plan host only call it.
#[test]
fn permission_mode_narrows_the_allowlist() {
    use AntigravityTool as T;
    let declared = [T::ReadFile, T::RunCommand, T::SearchWeb];
    let cases = [
        (
            PermissionMode::Plan,
            &declared[..],
            Ok(("plan", vec![T::ReadFile])),
        ),
        (
            PermissionMode::Bypass,
            &declared[..],
            Ok(("default", declared.to_vec())),
        ),
        (
            PermissionMode::Plan,
            &[T::WriteFile][..],
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
        let actual = AntigravityFence::new(mode, tools)
            .map(|fence| (fence.session_mode(), fence.tools().to_vec()));
        assert_eq!(actual, expected, "{mode} {tools:?}");
    }
}

// Covers: each permission kind approves in Bypass only when a kept tool
// covers it, never on Bypass alone (a command without `run_command` is
// refused even if agy ignored the agent file); Plan approves nothing;
// unclassified kinds fail closed.
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
        (K::Read, T::ReadFile),
        (K::Edit, T::WriteFile),
        (K::Edit, T::EditFile),
        (K::Delete, T::EditFile),
        (K::Move, T::WriteFile),
        (K::Execute, T::RunCommand),
        (K::Search, T::SearchWeb),
    ];
    let plan = fence(PermissionMode::Plan, T::ALL);
    for (kind, member) in cases {
        let request = request(Some(kind));
        let other = if member == T::RunCommand {
            T::ReadFile
        } else {
            T::RunCommand
        };
        for (tool, expected) in [
            (member, PermissionDecision::AllowOnce),
            (other, PermissionDecision::Reject),
        ] {
            assert_eq!(
                fence(PermissionMode::Bypass, &[tool]).decide(&request),
                expected,
                "kind {kind:?}, tool {tool:?}"
            );
        }
        assert_eq!(
            plan.decide(&request),
            PermissionDecision::Reject,
            "{kind:?}"
        );
    }
    let full = fence(PermissionMode::Bypass, T::ALL);
    for kind in [
        Some(K::Fetch),
        Some(K::Other),
        Some(K::Think),
        Some(K::SwitchMode),
        None,
    ] {
        assert_eq!(
            full.decide(&request(kind)),
            PermissionDecision::Reject,
            "{kind:?}"
        );
    }
}
