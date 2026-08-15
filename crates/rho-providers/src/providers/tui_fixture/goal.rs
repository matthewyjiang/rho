//! Goal command fixtures: `/goal` prompt detection and completion-condition
//! matching for the delegation, retry, blocked, and questionnaire flows.

use rho_sdk::model::{ContentBlock, Message, ModelRequest};

use super::last_user_text;

pub(super) const GOAL_RETRY_CONDITION: &str = "fixture goal retry";
pub(super) const GOAL_BLOCKED_CONDITION: &str = "fixture goal blocked";
pub(super) const GOAL_DELEGATION_CONDITION: &str = "fixture goal delegation";
pub(super) const GOAL_QUESTIONNAIRE_CONDITION: &str = "fixture goal background questionnaire";
pub(super) const GOAL_DELEGATION_RETRY_CONDITION: &str = "fixture goal delegation retry";
pub(super) const DELEGATION_REVIEW_RESPONSE: &str =
    "background agent completion received with delegated result (delivery 1)";

pub(super) fn is_goal_retry_prompt(prompt: &str) -> bool {
    prompt.contains("The user invoked Rho's `/goal` command")
        && prompt.contains(&format!("Goal:\n{GOAL_RETRY_CONDITION}"))
}

pub(super) fn is_goal_delegation_prompt(prompt: &str) -> bool {
    prompt.contains("The user invoked Rho's `/goal` command")
        && prompt.contains(&format!("Goal:\n{GOAL_DELEGATION_CONDITION}\n\n"))
}

pub(super) fn is_goal_questionnaire_prompt(prompt: &str) -> bool {
    prompt.contains("The user invoked Rho's `/goal` command")
        && prompt.contains(&format!("Goal:\n{GOAL_QUESTIONNAIRE_CONDITION}\n\n"))
}

pub(super) fn is_goal_delegation_retry_continuation(prompt: &str) -> bool {
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

pub(super) fn is_blocked_goal_evaluation(request: &ModelRequest<'_>) -> bool {
    evaluation_condition_is(request, GOAL_BLOCKED_CONDITION)
}

pub(super) fn is_goal_questionnaire_evaluation(request: &ModelRequest<'_>) -> bool {
    evaluation_condition_is(request, GOAL_QUESTIONNAIRE_CONDITION)
}

pub(super) fn is_delegation_retry_goal_evaluation(request: &ModelRequest<'_>) -> bool {
    evaluation_condition_is(request, GOAL_DELEGATION_RETRY_CONDITION)
}

pub(super) fn is_delegation_goal_evaluation(request: &ModelRequest<'_>) -> bool {
    evaluation_condition_is(request, GOAL_DELEGATION_CONDITION)
}

pub(super) fn delegation_result_was_reviewed(request: &ModelRequest<'_>) -> bool {
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
