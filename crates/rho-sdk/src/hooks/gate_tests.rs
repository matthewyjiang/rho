use pretty_assertions::assert_eq;

use super::*;

fn request(policy: HookPolicyOutcome) -> PreToolUseRequest {
    crate::hooks::testing::before_tool_use_request("bash", policy)
}

#[test]
fn only_a_denial_carries_a_reason() {
    let cases = [
        (HookDecision::Continue, false, None),
        (
            HookDecision::deny("hook `user:no-force-push` denied the command"),
            true,
            Some("hook `user:no-force-push` denied the command"),
        ),
    ];

    for (decision, is_deny, reason) in cases {
        assert_eq!(decision.is_deny(), is_deny, "{decision:?}");
        assert_eq!(decision.denial_reason(), reason, "{decision:?}");
    }
}

#[tokio::test]
async fn the_allow_all_gate_never_narrows_a_decision() {
    for policy in [HookPolicyOutcome::Allow, HookPolicyOutcome::RequireApproval] {
        assert_eq!(
            AllowAllGate.evaluate(request(policy)).await,
            HookDecision::Continue
        );
    }
}
