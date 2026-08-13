use crate::{
    model::{ModelError, ModelEvent, ModelResponse, ModelUsage},
    protocol::cost::parse_usd_micros,
};

use super::{
    convert::{emit_chat_reasoning_context, finalize_chat_assistant, ChatAssistantFinish},
    tool_calls::{ChatToolCallPolicy, RawChatToolCall},
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
    /// Output/reasoning token pairing from the latest usage payload that
    /// reported an output count, kept for the throughput carrier at finish.
    output_usage: Option<ReportedOutputUsage>,
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
            output_usage: None,
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
            self.usage_snapshot = Some(match self.usage_snapshot.take() {
                Some(snapshot) => merge_cumulative_usage(&snapshot, usage),
                None => usage,
            });
            let (output_tokens, reasoning_tokens) =
                extract_output_usage(value.get("usage").unwrap_or(&serde_json::Value::Null));
            if let Some(output_tokens) = output_tokens {
                self.output_usage = Some(ReportedOutputUsage {
                    output_tokens,
                    reasoning_tokens,
                });
            }
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

    /// Finalizes and emits the usage snapshot, throughput carrier, and retained
    /// reasoning context through `on_event`.
    pub(crate) fn finish(
        self,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let context = GenerationTokenContext {
            reasoning_streamed: self.reasoning_delta_streamed,
            hidden_reasoning_risk: self.hidden_reasoning_risk,
        };
        if let Some(event) =
            resolve_generation_output_tokens(self.output_usage, context).into_event()
        {
            on_event(event)?;
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

// Keep the raw 1.x carrier until rho-providers can raise its minimum rho-sdk
// version. Package verification must compile against the currently published SDK.
pub(crate) fn generation_output_tokens_event(tokens: u64) -> ModelEvent {
    ModelEvent::ProviderContext {
        kind: "rho_model_call_generation_output_tokens".into(),
        position: None,
        data: serde_json::json!({ "tokens": tokens }),
    }
}

fn generation_output_tokens_unavailable_event() -> ModelEvent {
    ModelEvent::ProviderContext {
        kind: "rho_model_call_generation_output_tokens".into(),
        position: None,
        data: serde_json::json!({ "tokens": null }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationOutputTokens {
    Unreported,
    Reported(u64),
    Unavailable,
}

impl GenerationOutputTokens {
    pub(crate) fn into_event(self) -> Option<ModelEvent> {
        match self {
            Self::Unreported => None,
            Self::Reported(tokens) => Some(generation_output_tokens_event(tokens)),
            Self::Unavailable => Some(generation_output_tokens_unavailable_event()),
        }
    }
}

/// Whether this call may have produced reasoning tokens the stream never
/// showed. Decides how to treat a usage payload that reports output tokens
/// without reasoning-token details when no reasoning deltas streamed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HiddenReasoningRisk {
    /// No serialized control asked the host to reason; treat aggregate output
    /// totals as visible-generation tokens.
    Unlikely,
    /// Reasoning was requested (or cannot be ruled out), so an aggregate total
    /// may hide off-wire reasoning whose wall time sat before the visible
    /// stream. Without reasoning-token details, report throughput as
    /// unavailable instead of an inflated rate.
    Possible,
}

/// Stream observations that decide which output-token count matches the
/// generation window measured by the runtime.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GenerationTokenContext {
    /// Whether any reasoning deltas streamed before this usage payload.
    pub(crate) reasoning_streamed: bool,
    pub(crate) hidden_reasoning_risk: HiddenReasoningRisk,
}

pub(crate) struct UsageReport {
    pub(crate) usage: ModelUsage,
    pub(crate) generation_output_tokens: GenerationOutputTokens,
}

fn extract_output_usage(usage: &serde_json::Value) -> (Option<u64>, Option<u64>) {
    for (tokens_key, details_key) in [
        ("output_tokens", "output_tokens_details"),
        ("completion_tokens", "completion_tokens_details"),
    ] {
        let Some(output_tokens) = usage.get(tokens_key).and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let reasoning_tokens = usage
            .get(details_key)
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(serde_json::Value::as_u64);
        return (Some(output_tokens), reasoning_tokens);
    }
    (None, None)
}

/// Output/reasoning token pairing from one usage payload.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReportedOutputUsage {
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_tokens: Option<u64>,
}

/// Picks the output-token count that matches the runtime's generation window.
///
/// The window opens at the first generated event, including reasoning deltas.
/// Reasoning that streamed therefore spent its wall time inside the window, so
/// the full output total is the matching numerator even when the host itemizes
/// reasoning tokens separately. Reasoning that stayed off the wire spent its
/// wall time before the window: subtract it when the host itemizes it, and
/// refuse to report a count when it might exist but cannot be separated.
pub(crate) fn resolve_generation_output_tokens(
    output_usage: Option<ReportedOutputUsage>,
    context: GenerationTokenContext,
) -> GenerationOutputTokens {
    let Some(output_usage) = output_usage else {
        return GenerationOutputTokens::Unreported;
    };
    if context.reasoning_streamed {
        return GenerationOutputTokens::Reported(output_usage.output_tokens);
    }
    match (output_usage.reasoning_tokens, context.hidden_reasoning_risk) {
        (Some(reasoning_tokens), _) => output_usage
            .output_tokens
            .checked_sub(reasoning_tokens)
            .map_or(
                GenerationOutputTokens::Unavailable,
                GenerationOutputTokens::Reported,
            ),
        (None, HiddenReasoningRisk::Unlikely) => GenerationOutputTokens::Unreported,
        (None, HiddenReasoningRisk::Possible) => GenerationOutputTokens::Unavailable,
    }
}

/// [`resolve_generation_output_tokens`] over a raw stream payload.
pub(crate) fn extract_generation_output_tokens(
    value: &serde_json::Value,
    context: GenerationTokenContext,
) -> GenerationOutputTokens {
    let Some(usage) = value.get("usage").filter(|usage| usage.is_object()) else {
        return GenerationOutputTokens::Unreported;
    };
    let (output_tokens, reasoning_tokens) = extract_output_usage(usage);
    resolve_generation_output_tokens(
        output_tokens.map(|output_tokens| ReportedOutputUsage {
            output_tokens,
            reasoning_tokens,
        }),
        context,
    )
}

pub(crate) fn extract_usage_report(
    value: &serde_json::Value,
    context: GenerationTokenContext,
) -> Option<UsageReport> {
    Some(UsageReport {
        usage: extract_usage(value)?,
        generation_output_tokens: extract_generation_output_tokens(value, context),
    })
}

/// Usage fields as the host reported them, before cache-bucket derivation.
///
/// Snapshots merge at this raw level: deriving `ModelUsage` per snapshot
/// would let a later snapshot's cache-adjusted input combine with cache
/// buckets retained from an earlier one, double-counting cached tokens.
#[derive(Clone, Copy, Debug, Default)]
struct RawUsage {
    /// Raw input total; cache reads and writes are subsets of this count.
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    total_tokens: Option<u64>,
    context_window: Option<u64>,
    cost_usd_micros: Option<u64>,
}

impl RawUsage {
    /// OpenAI reports cache reads and writes as subsets of the raw input
    /// count, while `ModelUsage` keeps the three input buckets disjoint.
    fn into_model_usage(self) -> ModelUsage {
        let input_tokens = self.input_tokens.map(|input| {
            input
                .saturating_sub(self.cache_read_tokens.unwrap_or_default())
                .saturating_sub(self.cache_write_tokens.unwrap_or_default())
        });
        ModelUsage {
            input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            total_tokens: self.total_tokens,
            context_window: self.context_window,
            cost_usd_micros: self.cost_usd_micros,
        }
    }
}

/// Field-wise cumulative merge: a later snapshot wins where it reports a
/// field, earlier totals survive where it does not.
fn merge_cumulative_usage(previous: &RawUsage, observed: RawUsage) -> RawUsage {
    RawUsage {
        input_tokens: observed.input_tokens.or(previous.input_tokens),
        output_tokens: observed.output_tokens.or(previous.output_tokens),
        cache_read_tokens: observed.cache_read_tokens.or(previous.cache_read_tokens),
        cache_write_tokens: observed.cache_write_tokens.or(previous.cache_write_tokens),
        total_tokens: observed.total_tokens.or(previous.total_tokens),
        context_window: observed.context_window.or(previous.context_window),
        cost_usd_micros: observed.cost_usd_micros.or(previous.cost_usd_micros),
    }
}

pub(crate) fn extract_usage(value: &serde_json::Value) -> Option<ModelUsage> {
    extract_raw_usage(value).map(RawUsage::into_model_usage)
}

fn extract_raw_usage(value: &serde_json::Value) -> Option<RawUsage> {
    let usage = value.get("usage").filter(|usage| usage.is_object())?;
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|v| v.as_u64());
    let (output_tokens, _) = extract_output_usage(usage);
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

    Some(RawUsage {
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

#[cfg(test)]
#[path = "stream_chat_tests.rs"]
mod stream_chat_tests;
