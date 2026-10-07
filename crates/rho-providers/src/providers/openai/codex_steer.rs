use serde_json::{json, Value};

use crate::model::{ContentBlock, Message, ModelError};
use crate::protocol::openai_responses::lower_codex_history_message;

use super::codex_continuation::{split_request_context, CodexContinuationCandidate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SteerMode {
    AutoContinuation,
    RequiredInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SteerMatch {
    Reuse,
    FullReplay,
}

#[derive(Debug)]
pub(super) struct PendingSteer {
    pub(super) request_properties: Value,
    pub(super) request_input: Vec<Value>,
    pub(super) steer_items: Vec<Value>,
    pub(super) mode: SteerMode,
}

impl PendingSteer {
    pub(super) fn matches(&self, candidate: &CodexContinuationCandidate) -> SteerMatch {
        if self.mode == SteerMode::RequiredInput {
            return SteerMatch::FullReplay;
        }
        if candidate.request_properties != self.request_properties {
            return SteerMatch::FullReplay;
        }
        let (input, context) = split_request_context(&candidate.input);
        let (request_input, request_context) = split_request_context(&self.request_input);
        // Auto-continuation is already running with the original context. A
        // changed projection cannot be silently dropped: unlike response.create
        // reuse sends no input frame to deliver it. Replay in that case.
        if context != request_context {
            return SteerMatch::FullReplay;
        }
        let Some(middle) = input
            .strip_prefix(request_input)
            .and_then(|remaining| remaining.strip_suffix(self.steer_items.as_slice()))
        else {
            return SteerMatch::FullReplay;
        };
        if middle.iter().any(blocks_auto_continuation) {
            return SteerMatch::FullReplay;
        }
        SteerMatch::Reuse
    }
}

fn blocks_auto_continuation(item: &Value) -> bool {
    if item.get("role").and_then(Value::as_str) == Some("user") {
        return true;
    }
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call_output" | "configuration_update")
    )
}

pub(super) fn steer_items(content: &[ContentBlock]) -> Result<Vec<Value>, ModelError> {
    lower_codex_history_message(&Message::User(content.to_vec()), &mut Vec::new(), None)
}

pub(super) fn steer_frame(
    response_id: &str,
    content: &[ContentBlock],
) -> Result<Value, ModelError> {
    let items = steer_items(content)?;
    Ok(json!({
        "type": "response.steer",
        "previous_response_id": response_id,
        "input": items,
    }))
}

pub(super) fn steer_event_type(value: &Value) -> Option<&str> {
    value
        .get("type")
        .and_then(Value::as_str)
        .filter(|event_type| event_type.starts_with("response.steer"))
}

pub(super) fn is_steer_pending_required_input(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("response.steer.pending")
        && value.get("reason").and_then(Value::as_str) == Some("waiting_for_required_input")
}

#[cfg(test)]
#[path = "codex_steer_tests.rs"]
mod tests;
