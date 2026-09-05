//! Non-stream completions: tool results, compaction, and prompt fallbacks.

use rho_sdk::{
    model::{ContentBlock, Message, ModelRequest, ModelResponse},
    ProviderError,
};

use super::{
    advisor, completed, current_turn_tool_results, edit, last_user_text, tool_result,
    tool_result_for_name, AGENTS_LIST_CALL_ID, BACKGROUND_AGENT_CALL_ID,
    BACKGROUND_CLAUDE_AGENT_CALL_ID, BACKGROUND_QUESTIONNAIRE_AGENT_CALL_ID,
    BACKGROUND_QUESTIONNAIRE_COMPLETION, CLAUDE_AGENT_CALL_ID, CLAUDE_AGENT_ERROR_CALL_ID,
    CONCURRENT_FAST_CALL_ID, CONCURRENT_SLOW_CALL_ID, HOVER_TOOL_CALL_ID, PROCESS_RAIL_CALL_ID,
    PROGRESS_CALL_ID, QUESTIONNAIRE_CALL_ID, SUBAGENT_RAIL_AGENT_CALL_ID, TOOL_CALL_ID,
};

pub(super) fn compaction(
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if is_compaction_request(request) {
        Some(completed("deterministic compacted conversation summary"))
    } else {
        None
    }
}

pub(super) fn intercept(
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if let Some(result) = tool_result_for_name(request, "skill") {
        let instruction = result
            .content
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default();
        return Some(completed(format!(
            "skill command loaded before model response: {instruction}"
        )));
    }
    if let Some(result) = tool_result(request, TOOL_CALL_ID) {
        return Some(completed(format!(
            "tool lifecycle complete with one result: {}",
            result.content.lines().next().unwrap_or_default()
        )));
    }
    if tool_result(request, HOVER_TOOL_CALL_ID).is_some() {
        return Some(completed("hover tool lifecycle complete"));
    }
    if let Some(text) = edit::completion_text(request) {
        return Some(completed(text));
    }
    if let (Some(slow), Some(fast)) = (
        tool_result(request, CONCURRENT_SLOW_CALL_ID),
        tool_result(request, CONCURRENT_FAST_CALL_ID),
    ) {
        return Some(completed(format!(
            "concurrent progress complete in model order: {}; {}",
            slow.content, fast.content
        )));
    }
    if let Some(result) = tool_result(request, PROGRESS_CALL_ID) {
        return Some(completed(format!(
            "progress tool lifecycle complete with one result: {}",
            result.content
        )));
    }
    if let Some(result) = tool_result(request, PROCESS_RAIL_CALL_ID) {
        let receipt = result.content.lines().next().unwrap_or_default();
        return Some(completed(format!(
            "process rail fixture dispatched: {receipt}"
        )));
    }
    if let Some(result) = tool_result(request, SUBAGENT_RAIL_AGENT_CALL_ID) {
        let receipt = result.content.lines().next().unwrap_or_default();
        return Some(completed(format!(
            "subagent rail fixture dispatched: {receipt}"
        )));
    }
    if let Some(result) = tool_result(request, BACKGROUND_AGENT_CALL_ID) {
        // Echo the spawn receipt so PTY scenarios can assert from screen text
        // that the tool resolved immediately with a start line, then end the
        // turn so completion arrives through automatic delivery.
        let receipt = result.content.lines().next().unwrap_or_default();
        return Some(completed(format!("background agent dispatched: {receipt}")));
    }
    if let Some(result) = tool_result(request, CLAUDE_AGENT_CALL_ID) {
        // Foreground Claude runs return the full completion snapshot as the tool
        // result. Echo a short marker so the parent turn ends cleanly after the
        // user-visible completion text is already on screen.
        let receipt = result.content.lines().next().unwrap_or_default();
        return Some(completed(format!("claude agent tool finished: {receipt}")));
    }
    if let Some(result) = tool_result(request, BACKGROUND_CLAUDE_AGENT_CALL_ID) {
        let receipt = result.content.lines().next().unwrap_or_default();
        return Some(completed(format!(
            "background claude agent dispatched: {receipt}"
        )));
    }
    if let Some(result) = tool_result(request, CLAUDE_AGENT_ERROR_CALL_ID) {
        // Foreground failures surface as tool errors; the fixture still ends the
        // parent turn so the PTY can observe the failed agent presentation.
        let receipt = result.content.lines().next().unwrap_or_default();
        return Some(completed(format!(
            "claude agent tool error observed: {receipt}"
        )));
    }
    if let Some(result) = tool_result(request, BACKGROUND_QUESTIONNAIRE_AGENT_CALL_ID) {
        let receipt = result.content.lines().next().unwrap_or_default();
        return Some(completed(format!(
            "background questionnaire agent dispatched: {receipt}"
        )));
    }
    if let Some(result) = tool_result(request, QUESTIONNAIRE_CALL_ID) {
        return Some(questionnaire_completion(request, result));
    }
    if tool_result(request, AGENTS_LIST_CALL_ID).is_some() {
        return Some(completed("agents list complete"));
    }
    if let Some(completion) = advisor::completion(request) {
        return Some(completion);
    }
    let prompt = last_user_text(request).unwrap_or_default();
    if is_agent_notification(&prompt) {
        return Some(completed(describe_agent_notification(request, &prompt)));
    }
    if prompt.starts_with("Continue working toward this goal:") {
        return Some(completed("goal continued before delegated agent finished"));
    }
    if prompt.starts_with("Resume the following goal after it was blocked") {
        return Some(completed(
            "verified that the fixture release is now published",
        ));
    }
    if prompt == "fixture steer detail" {
        return Some(completed(
            "steering applied exactly once: fixture steer detail",
        ));
    }
    None
}

fn questionnaire_completion(
    request: &ModelRequest<'_>,
    result: &rho_sdk::model::ToolResult,
) -> Result<ModelResponse, ProviderError> {
    let count = current_turn_tool_results(request)
        .filter(|result| result.id == QUESTIONNAIRE_CALL_ID)
        .count();
    let prompt = last_user_text(request).unwrap_or_default();
    if matches!(
        prompt.as_str(),
        "fixture child questionnaire" | "fixture delayed child questionnaire"
    ) {
        if result.content.contains("blue") {
            return completed(format!(
                "{BACKGROUND_QUESTIONNAIRE_COMPLETION}: {}",
                result.content
            ));
        }
        return completed(format!(
            "background agent questionnaire received wrong answer: {}",
            result.content
        ));
    }
    completed(format!(
        "questionnaire response observed exactly {count} time(s): {}",
        result.content
    ))
}

fn is_agent_notification(text: &str) -> bool {
    text.starts_with("[agent notification]")
        || (text.starts_with("[runtime notifications for session ")
            && text.contains("[agent notification]"))
}

/// Validates notification identity, terminal state, and delegated result, then
/// reports delivery count for exactly-once assertions in PTY scenarios.
fn describe_agent_notification(request: &ModelRequest<'_>, prompt: &str) -> String {
    let deliveries = request
        .messages
        .iter()
        .filter(|message| {
            matches!(
                message,
                Message::User(content) if content.iter().any(|block| matches!(
                    block,
                    ContentBlock::Text(text) if is_agent_notification(text)
                ))
            )
        })
        .count();
    if prompt.contains("(worker): ok")
        && (prompt.contains("assistant stream part one part two")
            || prompt.contains(BACKGROUND_QUESTIONNAIRE_COMPLETION))
    {
        if prompt.contains(BACKGROUND_QUESTIONNAIRE_COMPLETION) {
            format!("background agent questionnaire completion received (delivery {deliveries})")
        } else {
            format!(
                "background agent completion received with delegated result (delivery {deliveries})"
            )
        }
    } else if prompt.contains("(claude-planner): ok") && prompt.contains("rho-claude-e2e-ok") {
        format!("\n\nclaude-background-delivery-{deliveries}: delegated result received")
    } else {
        format!("unexpected agent notification payload: {prompt}")
    }
}

fn is_compaction_request(request: &ModelRequest<'_>) -> bool {
    matches!(
        request.messages.first(),
        Some(Message::System(message))
            if message.starts_with("Summarize the compacted conversation history")
    )
}
