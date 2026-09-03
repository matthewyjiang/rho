//! Fixture behavior for advisor mode: the executor's call and the review itself.
//!
//! Two models take part. The executor is asked to consult the advisor, and the
//! advisor runs as a separate one-shot request whose system prompt belongs to
//! the advisor internal agent. The fixture answers both sides so a scenario can
//! watch advice, or an advisor failure, travel back to the executor.

use rho_sdk::{
    model::{Message, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError, ProviderErrorKind, Retryability,
};

use super::{completed, completed_tool_call, last_user_text, tool_result};

const CALL_ID: &str = "tui-fixture-advisor";

/// Prompt that makes the executor consult the advisor once.
const PROMPT: &str = "fixture advisor";

/// Same, but the advisor model itself fails, so the tool result is an error.
const FAILURE_PROMPT: &str = "fixture advisor failure";

/// Opening of the advisor internal agent's system prompt, which is how an
/// advisor one-shot is told apart from the executor's own requests.
const SYSTEM_PROMPT_PREFIX: &str = "You are a senior advisor reviewing";

const GUIDANCE: &str = "advisor guidance: land the smallest change first";

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    _events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    if (prompt == PROMPT || prompt == FAILURE_PROMPT) && is_pending(request) {
        return Some(call());
    }
    None
}

fn is_pending(request: &ModelRequest<'_>) -> bool {
    tool_result(request, CALL_ID).is_none()
}

/// The executor's single `advisor` call. It takes no arguments.
fn call() -> Result<ModelResponse, ProviderError> {
    completed_tool_call(CALL_ID, "advisor", serde_json::json!({}))
}

/// Answers an advisor one-shot, or `None` when this is not one.
///
/// The transcript selects the outcome: it holds the prompt the user submitted,
/// so one fixture prompt asks for advice and another asks the advisor to fail.
pub(super) fn review(request: &ModelRequest<'_>) -> Option<Result<ModelResponse, ProviderError>> {
    let is_advisor = request.messages.iter().any(|message| {
        matches!(message, Message::System(text) if text.starts_with(SYSTEM_PROMPT_PREFIX))
    });
    if !is_advisor {
        return None;
    }
    let transcript = last_user_text(request).unwrap_or_default();
    Some(if transcript.contains(FAILURE_PROMPT) {
        Err(ProviderError::new(
            ProviderErrorKind::Other,
            "deterministic advisor model failure",
            Retryability::Permanent,
        ))
    } else {
        completed(GUIDANCE)
    })
}

/// Echoes the advisor's first line so a scenario can tell from screen text that
/// the advice, or the failure, reached the executor.
pub(super) fn completion(
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    let result = tool_result(request, CALL_ID)?;
    let outcome = if result.ok { "advice" } else { "error" };
    Some(completed(format!(
        "advisor consulted ({outcome}): {}",
        result.content.lines().next().unwrap_or_default()
    )))
}
