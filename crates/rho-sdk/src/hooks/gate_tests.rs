use pretty_assertions::assert_eq;

use crate::hooks::event::HookEventKind;

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

#[tokio::test]
async fn a_gate_sees_the_policy_outcome_it_may_narrow() {
    struct RecordingGate;

    impl PreToolUseGate for RecordingGate {
        fn evaluate(&self, request: PreToolUseRequest) -> HookGateFuture<'_> {
            let observed = request.policy();
            Box::pin(async move {
                match observed {
                    HookPolicyOutcome::RequireApproval => {
                        HookDecision::deny("denied before the prompt")
                    }
                    HookPolicyOutcome::Allow => HookDecision::Continue,
                }
            })
        }
    }

    let gate: Box<dyn PreToolUseGate> = Box::new(RecordingGate);

    assert_eq!(
        gate.evaluate(request(HookPolicyOutcome::Allow)).await,
        HookDecision::Continue
    );
    assert_eq!(
        gate.evaluate(request(HookPolicyOutcome::RequireApproval))
            .await,
        HookDecision::deny("denied before the prompt")
    );
}

#[test]
fn the_request_exposes_the_envelope_a_handler_receives() {
    let request = request(HookPolicyOutcome::Allow);

    assert_eq!(request.envelope().event(), HookEventKind::BeforeToolUse);
    assert_eq!(request.policy(), HookPolicyOutcome::Allow);
}
