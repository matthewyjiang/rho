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
            decision(answer, feedback, classifier_configured),
            PlanExitDecision {
                target,
                feedback: feedback.map(str::to_owned),
            }
        );
    }
}

// Covers: a host without a handoff slot returns a stop receipt instead of asking or approving.
// Owner: plan-exit host availability contract.
#[tokio::test]
async fn unavailable_host_reports_plan_without_requesting_input() {
    let tool = PlanExitTool {
        slot: None,
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
