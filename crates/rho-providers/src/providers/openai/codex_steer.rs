use serde_json::{json, Value};

use crate::model::{ContentBlock, Message, ModelError};
use crate::protocol::openai_responses::lower_codex_history_message;

use super::codex_continuation::CodexContinuationCandidate;

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
    #[allow(dead_code)]
    pub(super) previous_response_id: String,
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
        if !starts_with_items(&candidate.input, &self.request_input) {
            return SteerMatch::FullReplay;
        }
        if !ends_with_items(&candidate.input, &self.steer_items) {
            return SteerMatch::FullReplay;
        }
        let middle_end = candidate.input.len() - self.steer_items.len();
        let middle = &candidate.input[self.request_input.len()..middle_end];
        if middle.iter().any(blocks_auto_continuation) {
            return SteerMatch::FullReplay;
        }
        SteerMatch::Reuse
    }
}

fn starts_with_items(input: &[Value], prefix: &[Value]) -> bool {
    input.len() >= prefix.len() && input[..prefix.len()] == *prefix
}

fn ends_with_items(input: &[Value], suffix: &[Value]) -> bool {
    input.len() >= suffix.len() && input[input.len() - suffix.len()..] == *suffix
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
