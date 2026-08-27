use crate::model::{ModelError, ModelEvent, ModelResponse};

use super::{
    convert::{emit_chat_reasoning_context, finalize_chat_assistant, ChatAssistantFinish},
    tool_calls::{ChatToolCallPolicy, RawChatToolCall},
    usage::{
        extract_raw_usage, resolve_generation_output_tokens, GenerationTokenContext,
        HiddenReasoningRisk, RawUsage,
    },
};

#[cfg(test)]
use super::convert::{extract_response_text, ResponsesResponse};

const MAX_STREAM_BLOCK_INDEX: usize = 4096;

/// Accumulates one chat-completions stream into a model response.
///
/// Hosts differ in how they send assistant output: text and reasoning arrive
/// as deltas, tool calls arrive as indexed fragments, and some hosts repeat a
/// completed message snapshot on the final chunk. Feed every SSE line to
/// `handle_line`, then call `into_finish` once to normalize the accumulated
/// state without emitting side effects.
///
/// Reasoning deltas are retained and replayed to later turns as
/// `openai_chat_reasoning_content` provider context (Qwen/DeepSeek-style
/// `reasoning_content`). History conversion only replays that context to the
/// exact model that produced it, and hosts that do not know the field ignore
/// it, so emitting it for every OpenAI-chat-style provider stays safe.
///
/// Usage is not forwarded per chunk: hosts restate usage as running or final
/// snapshots (sometimes on several chunks, with non-monotonic input totals),
/// so `finish` publishes one merged snapshot and one throughput carrier after
/// the stream ends.
pub(crate) struct ChatStreamAccumulator {
    text: String,
    reasoning: String,
    /// Set only when a reasoning delta streamed; a completed message snapshot
    /// can fill `reasoning` without any reasoning wall time inside the
    /// measured generation window.
    reasoning_delta_streamed: bool,
    tool_calls: Vec<RawChatToolCall>,
    policy: ChatToolCallPolicy,
    hidden_reasoning_risk: HiddenReasoningRisk,
    /// Merged usage snapshot across the stream, kept raw (before cache-bucket
    /// derivation). Chat hosts restate usage as running or final totals rather
    /// than increments, and some restate a snapshot on several chunks, so
    /// publishing each report downstream would double-count. `finish` derives
    /// and publishes one `ModelUsage` snapshot plus one throughput carrier.
    usage_snapshot: Option<RawUsage>,
}

impl Default for ChatStreamAccumulator {
    fn default() -> Self {
        Self::new(ChatToolCallPolicy::Strict, HiddenReasoningRisk::Unlikely)
    }
}

impl ChatStreamAccumulator {
    pub(crate) fn new(
        policy: ChatToolCallPolicy,
        hidden_reasoning_risk: HiddenReasoningRisk,
    ) -> Self {
        Self {
            text: String::new(),
            reasoning: String::new(),
            reasoning_delta_streamed: false,
            tool_calls: Vec::new(),
            policy,
            hidden_reasoning_risk,
            usage_snapshot: None,
        }
    }

    /// Consumes one SSE line. Returns whether the line counts as stream activity.
    pub(crate) fn handle_line(
        &mut self,
        line: &str,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<bool, ModelError> {
        let Some(data) = sse_data(line) else {
            return Ok(false);
        };
        if data == "[DONE]" {
            return Ok(true);
        }
        let Some(value) = serde_json::from_str::<serde_json::Value>(data).ok() else {
            return Ok(false);
        };
        if let Some(usage) = extract_raw_usage(&value) {
            self.usage_snapshot = Some(
                self.usage_snapshot
                    .map_or(usage, |snapshot| snapshot.merge(usage)),
            );
        }
        let Some(choice) = value
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|choices| choices.first())
        else {
            return Ok(true);
        };
        let delta = choice.get("delta");
        if let Some(reasoning_delta) = delta
            .and_then(|v| {
                v.get("reasoning_content")
                    .or_else(|| v.get("reasoning"))
                    .or_else(|| v.get("reasoning_text"))
            })
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            self.reasoning_delta_streamed = true;
            self.reasoning.push_str(reasoning_delta);
            on_event(ModelEvent::ReasoningDelta(reasoning_delta.to_string()))?;
        }
        if let Some(content_delta) = delta
            .and_then(|v| v.get("content"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            on_event(ModelEvent::OutputDelta(content_delta.to_string()))?;
            self.text.push_str(content_delta);
        }
        let Some(delta_tool_calls) = delta
            .and_then(|v| v.get("tool_calls"))
            .and_then(|v| v.as_array())
        else {
            if let Some(message) = choice.get("message") {
                self.merge_completed_message(message);
            }
            return Ok(true);
        };

        for delta in delta_tool_calls {
            let index = delta.get("index").and_then(|v| v.as_u64()).map_or(
                Ok(self.tool_calls.len()),
                |index| {
                    usize::try_from(index).map_err(|_| {
                        ModelError::InvalidResponse(format!(
                            "stream block index {index} out of range"
                        ))
                    })
                },
            )?;
            if index > MAX_STREAM_BLOCK_INDEX {
                return Err(ModelError::InvalidResponse(format!(
                    "stream block index {index} out of range"
                )));
            }
            while self.tool_calls.len() <= index {
                self.tool_calls.push(RawChatToolCall::default());
            }
            let call = &mut self.tool_calls[index];
            let id = delta
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|id| !id.is_empty())
                .map(str::to_string);
            if let Some(id) = &id {
                call.id = Some(id.clone());
            }
            let name = delta
                .get("function")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .filter(|name| !name.is_empty())
                .map(str::to_string);
            if let Some(name) = &name {
                call.name = Some(name.clone());
            }
            let arguments_fragment = tool_call_arguments_fragment(
                delta.get("function").and_then(|v| v.get("arguments")),
            );
            if !arguments_fragment.is_empty() {
                call.arguments.push_str(&arguments_fragment);
            }
            if id.is_some() || name.is_some() || !arguments_fragment.is_empty() {
                on_event(ModelEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments: arguments_fragment,
                })?;
            }
        }
        // Some OpenAI-compatible hosts also put a completed message on the final
        // chunk. Prefer that snapshot when streamed tool fragments are incomplete.
        if let Some(message) = choice.get("message") {
            self.merge_completed_message(message);
        }
        Ok(true)
    }

    /// Pure finalization: builds response + reasoning without event side effects.
    pub(crate) fn into_finish(self) -> Result<ChatAssistantFinish, ModelError> {
        finalize_chat_assistant(self.text, self.reasoning, self.tool_calls, self.policy)
    }

    /// Finalizes and emits the usage snapshot, generation-output metric, and retained
    /// reasoning context through `on_event`.
    pub(crate) fn finish(
        self,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let context = GenerationTokenContext {
            reasoning_streamed: self.reasoning_delta_streamed,
            hidden_reasoning_risk: self.hidden_reasoning_risk,
        };
        let generation = resolve_generation_output_tokens(
            self.usage_snapshot.and_then(RawUsage::reported_output),
            context,
        );
        if let Some(tokens) = generation {
            on_event(ModelEvent::GenerationOutputTokens(tokens))?;
        }
        if let Some(usage) = self.usage_snapshot {
            on_event(ModelEvent::Usage(usage.into_model_usage()))?;
        }
        let finish = self.into_finish()?;
        emit_chat_reasoning_context(finish.reasoning_content, on_event)?;
        Ok(finish.response)
    }

    /// Fills gaps in the accumulated state from a completed message snapshot.
    fn merge_completed_message(&mut self, message: &serde_json::Value) {
        if self.text.is_empty() {
            if let Some(content) = message.get("content").and_then(|v| v.as_str()) {
                if !content.is_empty() {
                    self.text.push_str(content);
                }
            }
        }
        if self.reasoning.is_empty() {
            if let Some(reasoning) = message
                .get("reasoning_content")
                .or_else(|| message.get("reasoning"))
                .or_else(|| message.get("reasoning_text"))
                .and_then(|v| v.as_str())
                .filter(|text| !text.is_empty())
            {
                self.reasoning.push_str(reasoning);
            }
        }
        let Some(completed) = message.get("tool_calls").and_then(|v| v.as_array()) else {
            return;
        };
        for (fallback_index, completed_call) in completed.iter().enumerate() {
            let index = completed_call
                .get("index")
                .and_then(|v| v.as_u64())
                .and_then(|index| usize::try_from(index).ok())
                .unwrap_or(fallback_index);
            if index > MAX_STREAM_BLOCK_INDEX {
                continue;
            }
            while self.tool_calls.len() <= index {
                self.tool_calls.push(RawChatToolCall::default());
            }
            let call = &mut self.tool_calls[index];
            if call.id.as_ref().is_none_or(|id| id.is_empty()) {
                if let Some(id) = completed_call
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|id| !id.is_empty())
                {
                    call.id = Some(id.to_string());
                }
            }
            if call.name.as_ref().is_none_or(|name| name.is_empty()) {
                if let Some(name) = completed_call
                    .get("function")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .filter(|name| !name.is_empty())
                {
                    call.name = Some(name.to_string());
                }
            }
            if call.arguments.trim().is_empty() {
                let fragment = tool_call_arguments_fragment(
                    completed_call
                        .get("function")
                        .and_then(|v| v.get("arguments")),
                );
                if !fragment.is_empty() {
                    call.arguments = fragment;
                }
            }
        }
    }
}

/// Accepts both OpenAI string fragments and hosts that emit JSON values.
fn tool_call_arguments_fragment(arguments: Option<&serde_json::Value>) -> String {
    match arguments {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(value) if !value.is_null() => value.to_string(),
        _ => String::new(),
    }
}

pub(crate) fn line_decode_error(
    err: crate::provider_backend::line_decoder::LineDecodeError,
) -> ModelError {
    ModelError::InvalidResponse(format!("streamed response could not be decoded: {err}"))
}

pub(crate) fn sse_data(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("data:")?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

#[cfg(test)]
pub(crate) fn extract_sse_text(body: &str) -> Result<String, ModelError> {
    let mut text = String::new();
    for line in body.lines() {
        let Some(data) = sse_data(line) else {
            continue;
        };
        if data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        match value.get("type").and_then(|v| v.as_str()) {
            Some("response.output_text.delta") => {
                if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                    text.push_str(delta);
                }
            }
            Some("response.completed") if text.is_empty() => {
                if let Ok(response) =
                    serde_json::from_value::<ResponsesResponse>(value["response"].clone())
                {
                    return extract_response_text(response);
                }
            }
            _ => {}
        }
    }
    if text.is_empty() {
        Err(ModelError::InvalidResponse(format!(
            "missing response text in SSE: {body}"
        )))
    } else {
        Ok(text)
    }
}

#[cfg(test)]
#[path = "stream_chat_tests.rs"]
mod stream_chat_tests;
