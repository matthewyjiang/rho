use crate::{
    model::{ContentBlock, ModelError, ModelEvent, ModelResponse, ModelUsage},
    protocol::cost::parse_usd_micros,
};

use super::tool_calls::{finalize_chat_tool_calls, RawChatToolCall};

#[cfg(test)]
use super::convert::{extract_response_text, ResponsesResponse};

const MAX_STREAM_BLOCK_INDEX: usize = 4096;

/// Accumulates one chat-completions stream into a model response.
///
/// Hosts differ in how they send assistant output: text and reasoning arrive
/// as deltas, tool calls arrive as indexed fragments, and some hosts repeat a
/// completed message snapshot on the final chunk. Feed every SSE line to
/// `handle_line`, then call `finish` once to normalize the accumulated state.
///
/// Reasoning deltas are retained and replayed to later turns as
/// `openai_chat_reasoning_content` provider context (Qwen/DeepSeek-style
/// `reasoning_content`). History conversion only replays that context to the
/// exact model that produced it, and hosts that do not know the field ignore
/// it, so emitting it for every OpenAI-chat-style provider stays safe.
#[derive(Default)]
pub(crate) struct ChatStreamAccumulator {
    text: String,
    reasoning: String,
    tool_calls: Vec<RawChatToolCall>,
}

impl ChatStreamAccumulator {
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
        if let Some(usage) = extract_usage(&value) {
            on_event(ModelEvent::Usage(usage))?;
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

    /// Emits the retained reasoning context, then normalizes the accumulated
    /// state into the final response.
    pub(crate) fn finish(
        self,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        if !self.reasoning.is_empty() {
            on_event(ModelEvent::ProviderContext {
                kind: super::convert::OPENAI_CHAT_REASONING_CONTENT_KIND.into(),
                position: Some(0),
                data: serde_json::Value::String(self.reasoning),
            })?;
        }
        let mut blocks = Vec::new();
        if !self.text.is_empty() {
            blocks.push(ContentBlock::Text(self.text));
        }
        blocks.extend(
            finalize_chat_tool_calls(self.tool_calls)?
                .into_iter()
                .map(ContentBlock::ToolCall),
        );
        Ok(ModelResponse::Assistant(blocks))
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

pub(crate) fn extract_usage(value: &serde_json::Value) -> Option<ModelUsage> {
    let usage = value.get("usage")?;
    let raw_input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|v| v.as_u64());
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(|v| v.as_u64());
    let total_tokens = usage.get("total_tokens").and_then(|v| v.as_u64());
    let input_details = usage
        .get("input_tokens_details")
        .or_else(|| usage.get("prompt_tokens_details"));
    let cache_read_tokens = input_details
        .and_then(|v| {
            v.get("cached_tokens")
                .or_else(|| v.get("cache_read_tokens"))
                .or_else(|| v.get("cached_input_tokens"))
        })
        .and_then(|v| v.as_u64());
    let cache_write_tokens = input_details
        .and_then(|v| {
            v.get("cache_write_tokens")
                .or_else(|| v.get("cache_creation_input_tokens"))
                .or_else(|| v.get("cache_creation_tokens"))
        })
        .and_then(|v| v.as_u64());
    let context_window = usage
        .get("context_window")
        .or_else(|| usage.get("context_window_tokens"))
        .and_then(|v| v.as_u64());
    let reported_cost = [
        usage.get("cost_usd"),
        usage.get("estimated_cost_usd"),
        usage.get("cost"),
        usage.get("estimated_cost"),
    ]
    .into_iter()
    .flatten()
    .find_map(parse_usd_micros);
    let upstream_cost = usage
        .get("cost_details")
        .and_then(|details| details.get("upstream_inference_cost"))
        .and_then(parse_usd_micros);
    let cost_usd_micros = match (reported_cost, upstream_cost) {
        (Some(reported), Some(upstream)) => Some(reported.saturating_add(upstream)),
        (Some(reported), None) => Some(reported),
        (None, Some(upstream)) => Some(upstream),
        (None, None) => None,
    };

    // OpenAI reports cache reads and writes as subsets of the raw input count,
    // while ModelUsage keeps the three input buckets disjoint.
    let input_tokens = raw_input_tokens.map(|input| {
        input
            .saturating_sub(cache_read_tokens.unwrap_or_default())
            .saturating_sub(cache_write_tokens.unwrap_or_default())
    });

    Some(ModelUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens,
        context_window,
        cost_usd_micros,
    })
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
#[path = "stream_cost_tests.rs"]
mod stream_cost_tests;
