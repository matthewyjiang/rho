//! Plan proposals from composer, goal, and idle completion turns.

use rho_sdk::{
    model::{Message, ModelRequest, ModelResponse},
    ProviderError, ProviderErrorKind, Retryability,
};
use serde_json::json;

use super::{completed, completed_tool_call, tool_result};

const PROMPT: &str = "fixture plan exit";
const LONG_PROMPT: &str = "fixture plan exit long";
const FAIL_PROMPT: &str = "fixture plan exit fail";
const BACKGROUND_PROMPT: &str = "fixture plan exit background";
const HELD_PROMPT: &str = "fixture plan exit held";
const CHILD_PROMPT: &str = "fixture plan exit child";
const CHILD_RESULT: &str = "fixture plan child complete";
const AGENT_CALL_ID: &str = "tui-fixture-plan-agent";
const CALL_ID: &str = "tui-fixture-plan-exit";

pub(super) async fn intercept_child(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt != CHILD_PROMPT {
        return None;
    }
    // Release only after the PTY sees the parent dispatch receipt. This makes
    // the proposal originate in idle completion delivery, never in the parent.
    Some(
        match super::release::wait_for_release_or_cancel(
            ".rho-fixture-release-plan-child",
            &request.cancellation,
        )
        .await
        {
            Ok(()) => completed(CHILD_RESULT),
            Err(error) => Err(error),
        },
    )
}

/// Holds the proposal turn after the approval answer until the PTY releases it,
/// so a test can queue Alt+M while the approved turn is still running.
pub(super) async fn intercept_held(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt != HELD_PROMPT || tool_result(request, CALL_ID).is_none() {
        return None;
    }
    Some(
        match super::release::wait_for_release_or_cancel(
            ".rho-fixture-release-plan-held",
            &request.cancellation,
        )
        .await
        {
            Ok(()) => completed("fixture plan review complete"),
            Err(error) => Err(error),
        },
    )
}

pub(super) fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt == BACKGROUND_PROMPT {
        return Some(if tool_result(request, AGENT_CALL_ID).is_some() {
            completed("fixture plan background dispatched")
        } else {
            completed_tool_call(
                AGENT_CALL_ID,
                "agent",
                json!({"agent_id": "worker", "prompt": CHILD_PROMPT}),
            )
        });
    }
    let goal_proposal = prompt.contains("The user invoked Rho's `/goal` command")
        && prompt.contains("Goal:\nfixture plan exit\n\n");
    let idle_proposal = prompt.contains("[agent notification]") && prompt.contains(CHILD_RESULT);
    if matches!(prompt, PROMPT | LONG_PROMPT | FAIL_PROMPT | HELD_PROMPT)
        || goal_proposal
        || idle_proposal
    {
        return Some(if tool_result(request, CALL_ID).is_some() {
            if prompt == FAIL_PROMPT {
                Err(ProviderError::new(
                    ProviderErrorKind::Other,
                    "fixture plan proposal failed",
                    Retryability::Permanent,
                ))
            } else {
                completed("fixture plan review complete")
            }
        } else {
            // More rows than the default 10-row tool budget, ending with a
            // distinct step that must be visible before the user approves.
            let plan = if prompt == LONG_PROMPT {
                let mut plan = String::from("# Fixture plan\n\n");
                for step in 1..=12 {
                    plan.push_str(&format!("{step}. Inspect workspace component {step}.\n"));
                }
                plan.push_str("13. Verify the final safety step.");
                plan
            } else {
                "# Fixture plan\n\n1. Inspect the workspace.\n2. Implement the approved change.\n3. Verify the result.".into()
            };
            completed_tool_call(CALL_ID, "exit_plan_mode", json!({"plan": plan}))
        });
    }
    let approval_result = request.messages.iter().find_map(|message| match message {
        Message::ToolResult(result) if result.id == CALL_ID => Some(result.content.as_str()),
        _ => None,
    })?;
    // The automatic follow-up is a new user turn, with the rebuilt tool registry.
    Some(
        if request
            .tools
            .iter()
            .any(|tool| tool.name == "exit_plan_mode")
        {
            completed("fixture plan follow-up still has plan policy")
        } else if !prompt.starts_with("Implement the approved plan now.") {
            completed("fixture plan received unrelated continuation")
        } else {
            // Capture the model-visible approval result, not the host's decision
            // slot. The PTY checks that optional feedback survives into history.
            std::fs::write(".rho-fixture-plan-feedback", approval_result)
                .and_then(|()| std::fs::write(".rho-fixture-plan-implementation", prompt))
                .map_err(|error| {
                    ProviderError::new(
                        ProviderErrorKind::Other,
                        format!("plan fixture receipt: {error}"),
                        Retryability::Permanent,
                    )
                })
                .and_then(|()| completed("fixture plan implementation reached"))
        },
    )
}
