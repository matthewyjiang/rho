use std::{
    num::NonZeroUsize,
    sync::{atomic::AtomicBool, Arc},
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    tool::{tool_progress_channel, ToolContext, ToolInvocation},
    CancellationToken, ToolCallId,
};
use serde_json::json;

use super::*;

// Covers: offered decisions map to permission modes; unconfigured Auto fails closed.
// Owner: plan-exit answer policy.
#[test]
fn answers_map_to_approval_or_revision() {
    for (answer, classifier_configured, target, feedback) in [
        ("allow_edits", false, PermissionMode::AllowEdits, None),
        ("supervised", false, PermissionMode::Supervised, None),
        ("bypass", false, PermissionMode::Bypass, None),
        ("auto", true, PermissionMode::Auto, None),
        ("auto", false, PermissionMode::Plan, None),
        ("keep_planning", true, PermissionMode::Plan, None),
        (
            "keep_planning",
            false,
            PermissionMode::Plan,
            Some("include migration steps"),
        ),
    ] {
        assert_eq!(
            decision(answer, feedback, &approval_targets(classifier_configured)),
            PlanExitDecision {
                target,
                feedback: feedback.map(str::to_owned),
            }
        );
    }
}

// Covers: approval may switch modes only after the proposing turn completed
// while the session is still in plan mode; every other case keeps the mode
// Owner: plan-exit answer policy.
#[test]
fn decisions_apply_only_after_a_completed_turn_in_plan_mode() {
    let approve = PlanExitDecision {
        target: PermissionMode::AllowEdits,
        feedback: None,
    };
    let keep = PlanExitDecision {
        target: PermissionMode::Plan,
        feedback: None,
    };
    for (decision, turn, current, expected) in [
        (
            &approve,
            ProposalTurn::Completed,
            PermissionMode::Plan,
            PlanExitResolution::Apply(PermissionMode::AllowEdits),
        ),
        (
            &approve,
            ProposalTurn::Unfinished,
            PermissionMode::Plan,
            PlanExitResolution::Keep("plan approval discarded; turn did not complete"),
        ),
        (
            &approve,
            ProposalTurn::Completed,
            PermissionMode::Supervised,
            PlanExitResolution::Keep("plan approval discarded; session is no longer in plan mode"),
        ),
        (
            &keep,
            ProposalTurn::Completed,
            PermissionMode::Plan,
            PlanExitResolution::Keep("plan not approved; permission mode stays plan"),
        ),
    ] {
        assert_eq!(
            decision.resolve(turn, current),
            expected,
            "{turn:?} {current:?}"
        );
    }
}

// Covers: a host without a handoff slot returns a stop receipt instead of asking or approving.
// Owner: plan-exit host availability contract.
#[tokio::test]
async fn unavailable_host_reports_plan_without_requesting_input() {
    let tool = PlanExitTool {
        slot: None,
        prompt: PlanExitPrompt::Questionnaire,
        classifier_configured: Arc::new(AtomicBool::new(false)),
    };
    let (progress, _receiver) = tool_progress_channel(NonZeroUsize::MIN);
    let output = tool
        .call(
            ToolInvocation::new(
                ToolCallId::new(),
                json!({"plan":"# Plan\nInspect then implement"}),
            ),
            ToolContext::new(/*workspace*/ None, CancellationToken::new(), progress),
        )
        .await
        .unwrap();
    assert_eq!(
        output.structured_content(),
        Some(&json!({"status":"host_unavailable"}))
    );
}
