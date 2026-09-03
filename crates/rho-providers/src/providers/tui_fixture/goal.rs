//! Goal command fixtures: `/goal` prompt detection and completion-condition
//! matching for the delegation, retry, blocked, and questionnaire flows.

use std::sync::atomic::{AtomicUsize, Ordering};

use rho_sdk::{
    model::{ContentBlock, Message, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError, ProviderErrorKind, Retryability,
};

use super::{completed, completed_tool_call, last_user_text, tool_result};

static GOAL_RETRY_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
static GOAL_BLOCKED_EVALUATIONS: AtomicUsize = AtomicUsize::new(0);
static GOAL_DELEGATION_RETRY_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);

const GOAL_RETRY_CONDITION: &str = "fixture goal retry";
const GOAL_BLOCKED_CONDITION: &str = "fixture goal blocked";
const GOAL_DELEGATION_CONDITION: &str = "fixture goal delegation";
const GOAL_QUESTIONNAIRE_CONDITION: &str = "fixture goal background questionnaire";
const GOAL_DELEGATION_RETRY_CONDITION: &str = "fixture goal delegation retry";
const DELEGATION_REVIEW_RESPONSE: &str =
    "background agent completion received with delegated result (delivery 1)";

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    _events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    if is_goal_retry_prompt(prompt) {
        if GOAL_RETRY_ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
            return Some(Err(ProviderError::new(
                ProviderErrorKind::Unavailable,
                "deterministic transient goal turn failure",
                Retryability::Retryable,
            )));
        }
        return Some(completed(
            "fixture goal retry completed after reusing the original prompt",
        ));
    }
    if is_goal_delegation_retry_continuation(prompt) {
        if delegation_result_was_reviewed(request) {
            return Some(completed(
                "goal retry resumed after delegated agent finished",
            ));
        }
        if tool_result(request, super::GOAL_RETRY_AGENT_CALL_ID).is_none() {
            if GOAL_DELEGATION_RETRY_ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
                return Some(completed_tool_call(
                    super::GOAL_RETRY_AGENT_CALL_ID,
                    "agent",
                    serde_json::json!({
                        "agent_id": "worker",
                        "prompt": "fixture slow stream",
                        "background": true,
                    }),
                ));
            }
            return Some(completed(
                "goal retry started before delegated agent finished",
            ));
        }
        return Some(Err(ProviderError::new(
            ProviderErrorKind::Unavailable,
            "deterministic goal delegation retry failure",
            Retryability::Retryable,
        )));
    }
    if is_goal_questionnaire_prompt(prompt)
        && tool_result(request, super::BACKGROUND_QUESTIONNAIRE_AGENT_CALL_ID).is_none()
    {
        return Some(completed_tool_call(
            super::BACKGROUND_QUESTIONNAIRE_AGENT_CALL_ID,
            "agent",
            serde_json::json!({
                "agent_id": "worker",
                "prompt": "fixture delayed child questionnaire",
                "background": true,
            }),
        ));
    }
    if is_goal_delegation_prompt(prompt)
        && tool_result(request, super::BACKGROUND_AGENT_CALL_ID).is_none()
    {
        return Some(completed_tool_call(
            super::BACKGROUND_AGENT_CALL_ID,
            "agent",
            serde_json::json!({
                "agent_id": "worker",
                "prompt": "fixture stream",
                "background": true,
            }),
        ));
    }
    None
}

pub(super) fn intercept_response(
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if is_blocked_goal_evaluation(request) {
        let evaluation = if GOAL_BLOCKED_EVALUATIONS.fetch_add(1, Ordering::SeqCst) == 0 {
            r#"{"state":"Blocked","reason":"all fixture work is complete; publishing requires user authority","human_steps":[{"action":"publish the fixture release","reason":"requires the user's credentials"}]}"#
        } else {
            r#"{"state":"Met","reason":"the fixture release is now published","human_steps":[]}"#
        };
        return Some(completed(evaluation));
    }
    if is_goal_questionnaire_evaluation(request) {
        return Some(completed(
            r#"{"state":"Met","reason":"the delegated questionnaire was answered","human_steps":[]}"#,
        ));
    }
    if is_delegation_retry_goal_evaluation(request) {
        let reviewed = last_user_text(request)
            .is_some_and(|prompt| prompt.contains(DELEGATION_REVIEW_RESPONSE));
        let evaluation = if reviewed {
            r#"{"state":"Met","reason":"the delegated retry result was reviewed","human_steps":[]}"#
        } else {
            r#"{"state":"Unmet","reason":"delegate work before continuing","human_steps":[]}"#
        };
        return Some(completed(evaluation));
    }
    if is_delegation_goal_evaluation(request) {
        let reviewed = last_user_text(request)
            .is_some_and(|prompt| prompt.contains(DELEGATION_REVIEW_RESPONSE));
        let evaluation = if reviewed {
            r#"{"state":"Met","reason":"the delegated result was reviewed","human_steps":[]}"#
        } else {
            r#"{"state":"Unmet","reason":"the delegated result still needs review","human_steps":[]}"#
        };
        return Some(completed(evaluation));
    }
    None
}

fn is_goal_retry_prompt(prompt: &str) -> bool {
    prompt.contains("The user invoked Rho's `/goal` command")
        && prompt.contains(&format!("Goal:\n{GOAL_RETRY_CONDITION}"))
}

fn is_goal_delegation_prompt(prompt: &str) -> bool {
    prompt.contains("The user invoked Rho's `/goal` command")
        && prompt.contains(&format!("Goal:\n{GOAL_DELEGATION_CONDITION}\n\n"))
}

fn is_goal_questionnaire_prompt(prompt: &str) -> bool {
    prompt.contains("The user invoked Rho's `/goal` command")
        && prompt.contains(&format!("Goal:\n{GOAL_QUESTIONNAIRE_CONDITION}\n\n"))
}

fn is_goal_delegation_retry_continuation(prompt: &str) -> bool {
    prompt.starts_with("Continue working toward this goal:")
        && prompt.contains(GOAL_DELEGATION_RETRY_CONDITION)
}

/// The request carries the goal-completion evaluator system prompt.
fn is_goal_evaluation(request: &ModelRequest<'_>) -> bool {
    request.messages.iter().any(|message| {
        matches!(
            message,
            Message::System(prompt) if prompt.contains("conservative goal-completion evaluator")
        )
    })
}

fn evaluation_condition_is(request: &ModelRequest<'_>, condition: &str) -> bool {
    is_goal_evaluation(request)
        && last_user_text(request)
            .is_some_and(|prompt| prompt.contains(&format!("Completion condition:\n{condition}")))
}

fn is_blocked_goal_evaluation(request: &ModelRequest<'_>) -> bool {
    evaluation_condition_is(request, GOAL_BLOCKED_CONDITION)
}

fn is_goal_questionnaire_evaluation(request: &ModelRequest<'_>) -> bool {
    evaluation_condition_is(request, GOAL_QUESTIONNAIRE_CONDITION)
}

fn is_delegation_retry_goal_evaluation(request: &ModelRequest<'_>) -> bool {
    evaluation_condition_is(request, GOAL_DELEGATION_RETRY_CONDITION)
}

fn is_delegation_goal_evaluation(request: &ModelRequest<'_>) -> bool {
    evaluation_condition_is(request, GOAL_DELEGATION_CONDITION)
}

fn delegation_result_was_reviewed(request: &ModelRequest<'_>) -> bool {
    request.messages.iter().any(|message| {
        message
            .completed_assistant_content()
            .is_some_and(|content| {
                content.iter().any(|block| {
                    matches!(
                        block,
                        ContentBlock::Text(text) if text.contains(DELEGATION_REVIEW_RESPONSE)
                    )
                })
            })
    })
}
