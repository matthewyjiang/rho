//! Codex Responses-API SSE protocol handling.
//!
//! Parses `response.*` SSE events (and WebSocket frames that reuse the same
//! payloads) into model events and one completed response.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    model::{
        ContentBlock, ImageContent, ModelError, ModelEvent, ModelResponse,
        ProviderReportedErrorKind,
    },
    provider_backend::line_stream::collect_line_stream,
};
use rho_sdk::model::ToolCall;

use super::compact::COMPACTION_OUTPUT_ITEM_KIND;
use super::convert::{extract_response_text, ResponsesResponse};
use super::image_generation::{
    image_from_generation_call, is_image_generation_call, slim_image_generation_item,
};
use super::stream::{line_decode_error, sse_data};
use super::usage::{extract_usage_report, GenerationTokenContext, HiddenReasoningRisk};

/// Max chars for a single search/url detail string in activity previews.
const DETAIL_MAX_CHARS: usize = 80;
/// Max chars per query when rendering multi-query search previews.
const QUERY_MAX_CHARS: usize = 48;

/// The Responses transports that share terminal-failure handling.
///
/// The transports classify a bare `error` event (one carrying no code/type or
/// message) differently: the WebSocket transport reports `websocket_error`,
/// which maps to a retryable [`ProviderReportedErrorKind::Unavailable`], while
/// the HTTP SSE transport reports `response_error`, which maps to a permanent
/// `InvalidResponse` kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexTransport {
    HttpSse,
    WebSocket,
}

impl CodexTransport {
    /// `(error_type, message)` used when a bare `error` event carries neither.
    fn bare_error_fallback(self) -> (&'static str, &'static str) {
        match self {
            Self::HttpSse => ("response_error", "error event received"),
            Self::WebSocket => ("websocket_error", "websocket error event received"),
        }
    }
}

/// Terminal failure payloads shared by the SSE and WebSocket Responses
/// transports.
///
/// Returns `(error_type, message)` for `error`, `response.failed`, and
/// `response.incomplete` events so both transports surface the provider's own
/// error instead of an empty-content diagnostic. A bare `error` event with no
/// code/type and message falls back to `transport`'s naming.
fn codex_terminal_failure(
    value: &serde_json::Value,
    transport: CodexTransport,
) -> Option<(String, String)> {
    let event_type = value.get("type").and_then(|v| v.as_str())?;
    match event_type {
        "error" => {
            let (fallback_error_type, fallback_message) = transport.bare_error_fallback();
            // The provider may nest fields under `error` or place `code` and
            // `message` at the event level. A bare `{type: "error"}` event has
            // neither, so it falls back to the transport's naming. The
            // top-level `type` is the event discriminator and must never be
            // read as an error code.
            let nested = value.get("error").filter(|error| error.is_object());
            Some((
                nested
                    .and_then(|error| error.get("code"))
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        nested
                            .and_then(|error| error.get("type"))
                            .and_then(|v| v.as_str())
                    })
                    .or_else(|| value.get("code").and_then(|v| v.as_str()))
                    .unwrap_or(fallback_error_type)
                    .to_string(),
                nested
                    .and_then(|error| error.get("message"))
                    .and_then(|v| v.as_str())
                    .or_else(|| value.get("message").and_then(|v| v.as_str()))
                    .unwrap_or(fallback_message)
                    .to_string(),
            ))
        }
        "response.failed" => {
            let error = value
                .get("response")
                .and_then(|response| response.get("error"));
            Some((
                error
                    .and_then(|error| {
                        error
                            .get("code")
                            .and_then(|v| v.as_str())
                            .or_else(|| error.get("type").and_then(|v| v.as_str()))
                    })
                    .unwrap_or("response_failed")
                    .to_string(),
                error
                    .and_then(|error| error.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("response.failed event received")
                    .to_string(),
            ))
        }
        "response.incomplete" if !is_steered_incomplete(value) => {
            let reason = incomplete_reason(value).unwrap_or("unknown");
            Some((
                "response_incomplete".to_string(),
                format!("incomplete response returned, reason: {reason}"),
            ))
        }
        _ => None,
    }
}

/// Map an OpenAI Responses error code/type to the retryability-kind surface.
///
/// The Anthropic transport has its own error-code vocabulary and its own
/// mapper: `anthropic_error_kind` in `protocol/anthropic_messages/stream.rs`.
/// Keep the two in sync in spirit (rate limit / unavailable / timeout are
/// retryable, everything else is permanent) but do not merge them; the code
/// strings differ per provider.
pub(crate) fn provider_reported_kind(error_type: &str) -> ProviderReportedErrorKind {
    match error_type {
        "rate_limit_exceeded" => ProviderReportedErrorKind::RateLimit,
        "server_error" | "service_unavailable" | "websocket_error" | "server_is_overloaded" => {
            ProviderReportedErrorKind::Unavailable
        }
        "timeout" | "request_timeout" => ProviderReportedErrorKind::Timeout,
        _ => ProviderReportedErrorKind::InvalidResponse,
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct CodexSseResponse {
    pub(crate) response: ModelResponse,
    pub(crate) response_id: Option<String>,
    pub(crate) service_tier: Option<String>,
    pub(crate) steered: bool,
}

fn incomplete_reason(value: &serde_json::Value) -> Option<&str> {
    value
        .get("response")
        .and_then(|response| response.get("incomplete_details"))
        .and_then(|details| details.get("reason"))
        .and_then(|v| v.as_str())
}

pub(crate) fn is_steered_incomplete(value: &serde_json::Value) -> bool {
    value.get("type").and_then(|v| v.as_str()) == Some("response.incomplete")
        && incomplete_reason(value) == Some("steered")
}

pub(crate) fn is_codex_turn_complete(value: &serde_json::Value) -> bool {
    value.get("type").and_then(|v| v.as_str()) == Some("response.completed")
        || is_steered_incomplete(value)
}

pub(crate) async fn collect_codex_sse_response(
    response: reqwest::Response,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
) -> Result<CodexSseResponse, ModelError> {
    let mut state = CodexSseState::default();
    collect_line_stream(response, line_decode_error, |line| {
        handle_codex_sse_line(line, &mut state, on_event)
    })
    .await?;
    state.into_response()
}

fn extract_reasoning_delta(value: &serde_json::Value) -> Option<String> {
    for key in [
        "delta",
        "text",
        "content",
        "summary",
        "reasoning",
        "reasoning_text",
    ] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }
    }
    for key in ["delta", "text", "content", "summary"] {
        if let Some(text) = value
            .get("item")
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_str())
        {
            return Some(text.to_string());
        }
    }
    None
}

fn is_reasoning_summary_event(event_type: &str) -> bool {
    event_type.contains("reasoning_summary") || event_type.contains("reasoning.summary")
}

#[derive(Default)]
pub(crate) struct CodexSseState {
    pub(crate) text: String,
    pub(crate) completed_text: Option<String>,
    pub(crate) tool_calls: Vec<ToolCall>,
    pub(crate) images: Vec<ImageContent>,
    pub(crate) response_id: Option<String>,
    pub(crate) service_tier: Option<String>,
    pub(crate) output_items: Vec<serde_json::Value>,
    /// True after a `response.completed` event was applied.
    completed: bool,
    /// True when the original response ended because it was steered.
    pub(crate) steered: bool,
    /// Provider `response.status` from the completed envelope, when present.
    response_status: Option<String>,
    /// Distinct SSE event `type` values observed on this stream.
    event_types: BTreeSet<String>,
    /// Tool-call argument text already published as [`ModelEvent::ToolCallDelta`],
    /// keyed by provider output index.
    ///
    /// Argument completion events restate the whole argument string. Publishing
    /// only the part that never streamed keeps live previews complete for
    /// responses that carry arguments without incremental deltas, while never
    /// re-appending text consumers already hold.
    published_tool_arguments: BTreeMap<usize, String>,
    /// Hosted-activity keys already published from `output_item.done`.
    ///
    /// `response.completed` may restate the same `output` items when the
    /// stream carried no text or function calls yet; skip those keys so
    /// WebSearch / HostedToolActivity are not dual-emitted.
    emitted_activity_keys: BTreeSet<String>,
    /// True once a non-empty reasoning or reasoning-summary delta streamed.
    /// Decides whether reasoning wall time sits inside the generation window
    /// when the completed usage payload is converted to a throughput count.
    reasoning_streamed: bool,
}

impl CodexSseState {
    pub(crate) fn into_response(self) -> Result<CodexSseResponse, ModelError> {
        let has_text = !self.text.is_empty()
            || self
                .completed_text
                .as_ref()
                .is_some_and(|text| !text.is_empty());
        if !has_text && self.tool_calls.is_empty() && self.images.is_empty() {
            return Err(self.missing_response_content_error());
        }

        let mut blocks = Vec::new();
        let text = if self.text.is_empty() {
            self.completed_text.unwrap_or_default()
        } else {
            self.text
        };
        if !text.is_empty() {
            blocks.push(ContentBlock::Text(text));
        }
        blocks.extend(self.images.into_iter().map(ContentBlock::Image));
        blocks.extend(self.tool_calls.into_iter().map(ContentBlock::ToolCall));
        Ok(CodexSseResponse {
            response: ModelResponse::Assistant(blocks),
            response_id: self.response_id,
            service_tier: self.service_tier,
            steered: self.steered,
        })
    }

    /// Build a debug-friendly empty-content error from the stream summary.
    ///
    /// Includes only structural fields (ids, statuses, item/event types). Does
    /// not attach raw SSE payloads or item bodies, which may carry user data.
    fn missing_response_content_error(&self) -> ModelError {
        ModelError::InvalidResponse(format!(
            "missing response content in SSE ({})",
            self.empty_content_diagnostic()
        ))
    }

    fn empty_content_diagnostic(&self) -> String {
        let mut parts = Vec::new();
        parts.push(format!("completed={}", self.completed));
        if let Some(response_id) = &self.response_id {
            parts.push(format!("response_id={response_id}"));
        }
        if let Some(status) = &self.response_status {
            parts.push(format!("status={status}"));
        }
        if let Some(service_tier) = &self.service_tier {
            parts.push(format!("service_tier={service_tier}"));
        }
        parts.push(format!("streamed_text_chars={}", self.text.chars().count()));
        parts.push(format!("images={}", self.images.len()));
        parts.push(format!("tool_calls={}", self.tool_calls.len()));
        let item_types = summarize_output_item_types(&self.output_items);
        if item_types.is_empty() {
            parts.push("output_items=none".into());
        } else {
            parts.push(format!("output_items=[{}]", item_types.join(", ")));
        }
        if self.event_types.is_empty() {
            parts.push("events=none".into());
        } else {
            parts.push(format!(
                "events=[{}]",
                self.event_types
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        parts.join("; ")
    }
}

/// Stable, count-collapsed list of output item `type` values for diagnostics.
fn summarize_output_item_types(items: &[serde_json::Value]) -> Vec<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for item in items {
        let kind = item
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        *counts.entry(kind.to_owned()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(kind, count)| {
            if count == 1 {
                kind
            } else {
                format!("{kind}:{count}")
            }
        })
        .collect()
}

/// Hosted-search item shapes on Responses streams.
///
/// - `web_search_call` → [`ModelEvent::WebSearch`]
/// - `x_search_call` → hosted activity `x_search`
/// - `custom_tool_call` whose `call_id` is in the `xs_call-` family → hosted
///   activity `x_search` (xAI emits hosted X subtools this way, e.g.
///   `x_keyword_search` with `call_id: "xs_call-…"`)
///
/// Classification is separate from detail formatting so non-search items never
/// pay argument JSON probing.
fn extract_codex_search_activity(item: &serde_json::Value) -> Option<ModelEvent> {
    let kind = classify_codex_search_item(item)?;
    let detail = extract_codex_search_detail(item).unwrap_or_default();
    Some(match kind {
        CodexSearchKind::Web => ModelEvent::WebSearch(detail),
        CodexSearchKind::X => ModelEvent::HostedToolActivity {
            name: "x_search".into(),
            detail,
        },
    })
}

#[derive(Clone, Copy)]
enum CodexSearchKind {
    Web,
    X,
}

/// `custom_tool_call.call_id` prefix used by hosted x_search subtool invocations.
const HOSTED_X_SEARCH_CALL_ID_PREFIX: &str = "xs_call-";

fn classify_codex_search_item(item: &serde_json::Value) -> Option<CodexSearchKind> {
    match item.get("type").and_then(|value| value.as_str())? {
        "web_search_call" => Some(CodexSearchKind::Web),
        "x_search_call" => Some(CodexSearchKind::X),
        "custom_tool_call" if is_hosted_x_search_custom_call(item) => Some(CodexSearchKind::X),
        _ => None,
    }
}

fn is_hosted_x_search_custom_call(item: &serde_json::Value) -> bool {
    item.get("call_id")
        .and_then(|call_id| call_id.as_str())
        .is_some_and(|call_id| call_id.starts_with(HOSTED_X_SEARCH_CALL_ID_PREFIX))
}

/// Stable key for a search output item, used to dedupe stream vs completed paths.
///
/// Prefer the wire `id`. Fall back to type+detail only when the item has no id,
/// using the already-built event so detail is not extracted twice.
fn hosted_activity_key(item: &serde_json::Value, event: &ModelEvent) -> String {
    if let Some(id) = item
        .get("id")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
    {
        return id.to_owned();
    }
    let kind = item
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("search");
    let detail = match event {
        ModelEvent::WebSearch(detail) => detail.as_str(),
        ModelEvent::HostedToolActivity { detail, .. } => detail.as_str(),
        _ => "",
    };
    format!("{kind}:{detail}")
}

fn emit_codex_search_activity(
    item: &serde_json::Value,
    state: &mut CodexSseState,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
) -> Result<(), ModelError> {
    let Some(event) = extract_codex_search_activity(item) else {
        return Ok(());
    };
    emit_hosted_activity(item, event, state, on_event)
}

fn emit_image_generation_activity(
    item: &serde_json::Value,
    state: &mut CodexSseState,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
) -> Result<(), ModelError> {
    if !is_image_generation_call(item) {
        return Ok(());
    }
    let detail = item
        .get("prompt")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(|value| truncate_detail(value, DETAIL_MAX_CHARS))
        .unwrap_or_default();
    emit_hosted_activity(
        item,
        ModelEvent::HostedToolActivity {
            name: "image_generation".into(),
            detail,
        },
        state,
        on_event,
    )
}

fn persist_image_generation_item(
    item: &serde_json::Value,
    position: Option<usize>,
    state: &mut CodexSseState,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
) -> Result<(), ModelError> {
    if !is_image_generation_call(item) {
        return Ok(());
    }
    let Some(image) = image_from_generation_call(item) else {
        return Ok(());
    };
    state.images.push(image);
    emit_output_item_replay(slim_image_generation_item(item), position, on_event)
}

fn emit_hosted_activity(
    item: &serde_json::Value,
    event: ModelEvent,
    state: &mut CodexSseState,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
) -> Result<(), ModelError> {
    let key = hosted_activity_key(item, &event);
    if !state.emitted_activity_keys.insert(key) {
        return Ok(());
    }
    if let Some(on_event) = on_event.as_mut() {
        on_event(event)?;
    }
    Ok(())
}

fn emit_output_item_replay(
    data: serde_json::Value,
    position: Option<usize>,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
) -> Result<(), ModelError> {
    if let Some(on_event) = on_event.as_mut() {
        on_event(ModelEvent::ProviderContext {
            kind: COMPACTION_OUTPUT_ITEM_KIND.into(),
            position,
            data,
        })?;
    }
    Ok(())
}

/// Bare semantic detail for a hosted search card (shown as a child fact).
///
/// Returns payload values only — no English chrome like `for` / `opened` /
/// `found`. The TUI owns presentation (verb header + optional detail fact +
/// `finished`).
///
/// Codex-style items put fields under `action`. xAI emits hosted X tools as
/// `custom_tool_call` with JSON in `input`. Older shapes may use `x_search_call`
/// and `arguments`. Empty detail is fine: the activity still emits.
fn extract_codex_search_detail(item: &serde_json::Value) -> Option<String> {
    item.get("action")
        .and_then(detail_from_search_action)
        .or_else(|| detail_from_search_arguments(item))
}

fn detail_from_search_action(action: &serde_json::Value) -> Option<String> {
    detail_from_search_payload(action)
        .or_else(|| first_nonempty_detail_field(action, &["url", "pattern"]))
}

fn detail_from_search_arguments(item: &serde_json::Value) -> Option<String> {
    let arguments = item
        .get("arguments")
        .or_else(|| item.get("input"))
        .and_then(|value| value.as_str())?;
    let args: serde_json::Value = serde_json::from_str(arguments).ok()?;
    if let Some(detail) = detail_from_search_payload(&args) {
        return Some(detail);
    }
    first_nonempty_detail_field(&args, &["post_id", "username", "url", "prompt"])
}

fn first_nonempty_detail_field(payload: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        payload
            .get(*key)
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(|value| truncate_detail(value, DETAIL_MAX_CHARS))
    })
}

fn detail_from_search_payload(payload: &serde_json::Value) -> Option<String> {
    if let Some(query) = payload
        .get("query")
        .and_then(|query| query.as_str())
        .filter(|query| !query.is_empty())
    {
        return Some(truncate_detail(query, DETAIL_MAX_CHARS));
    }
    let queries = payload
        .get("queries")
        .and_then(|queries| queries.as_array())?
        .iter()
        .filter_map(|query| query.as_str())
        .filter(|query| !query.is_empty())
        .collect::<Vec<_>>();
    if queries.is_empty() {
        return None;
    }
    let mut rendered = queries
        .iter()
        .take(3)
        .map(|query| truncate_detail(query, QUERY_MAX_CHARS))
        .collect::<Vec<_>>();
    if queries.len() > rendered.len() {
        rendered.push(format!("{} more", queries.len() - rendered.len()));
    }
    Some(rendered.join(" · "))
}

fn codex_output_item_key(item: &serde_json::Value) -> Option<(&str, &str)> {
    let kind = item.get("type").and_then(|value| value.as_str())?;
    let id = item
        .get("id")
        .or_else(|| item.get("call_id"))
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())?;
    Some((kind, id))
}

fn codex_output_item_was_processed(state: &CodexSseState, item: &serde_json::Value) -> bool {
    let key = codex_output_item_key(item);
    state.output_items.iter().any(|processed| {
        key.is_some_and(|key| codex_output_item_key(processed) == Some(key)) || processed == item
    })
}

fn truncate_detail(value: &str, max_chars: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

/// Stream index a tool-call event belongs to.
///
/// Responses events carry `output_index`; a stream that omits it addresses the
/// call currently being assembled, which is the next one to complete.
fn tool_call_index(value: &serde_json::Value, state: &CodexSseState) -> usize {
    value
        .get("output_index")
        .and_then(|index| index.as_u64())
        .and_then(|index| usize::try_from(index).ok())
        .unwrap_or(state.tool_calls.len())
}

/// Argument text for `index` that has not been published yet, recording it as
/// published.
///
/// Returns `None` when nothing is left to publish, or when `arguments` does not
/// extend what already streamed, so a restatement that diverges mid-call can
/// never corrupt a consumer's argument buffer.
fn unpublished_tool_arguments(
    state: &mut CodexSseState,
    index: usize,
    arguments: &str,
) -> Option<String> {
    let published = state.published_tool_arguments.entry(index).or_default();
    let unpublished = arguments.strip_prefix(published.as_str())?.to_string();
    if unpublished.is_empty() {
        return None;
    }
    published.push_str(&unpublished);
    Some(unpublished)
}

fn extract_codex_function_call(item: &serde_json::Value) -> Result<Option<ToolCall>, ModelError> {
    if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
        return Ok(None);
    }
    let name = item
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ModelError::InvalidResponse("function_call missing name".into()))?
        .to_string();
    let id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ModelError::InvalidResponse(format!("function_call {name} missing call_id"))
        })?
        .to_string();
    let arguments = match item.get("arguments") {
        None => serde_json::json!({}),
        Some(serde_json::Value::String(text)) => serde_json::from_str(text).map_err(|e| {
            ModelError::InvalidResponse(format!("invalid function_call arguments for {name}: {e}"))
        })?,
        Some(value @ serde_json::Value::Object(_)) => value.clone(),
        Some(_) => {
            return Err(ModelError::InvalidResponse(format!(
                "tool call arguments for {name} are not a JSON object"
            )));
        }
    };
    if !arguments.is_object() {
        return Err(ModelError::InvalidResponse(format!(
            "tool call arguments for {name} are not a JSON object"
        )));
    }
    Ok(Some(ToolCall {
        id,
        name,
        arguments,
    }))
}

pub(crate) fn handle_codex_sse_line(
    line: &str,
    state: &mut CodexSseState,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
) -> Result<bool, ModelError> {
    let Some(data) = sse_data(line) else {
        return Ok(false);
    };
    if data == "[DONE]" {
        return Ok(true);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return Ok(false);
    };
    handle_codex_sse_value(&value, state, on_event, CodexTransport::HttpSse)
}

/// Core of [`handle_codex_sse_line`] for callers that already hold a parsed
/// event payload (the Codex websocket transport), avoiding a serialize and
/// re-parse round-trip per streamed event.
pub(crate) fn handle_codex_sse_value(
    value: &serde_json::Value,
    state: &mut CodexSseState,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
    transport: CodexTransport,
) -> Result<bool, ModelError> {
    let event_type = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !event_type.is_empty() && !state.event_types.contains(event_type) {
        state.event_types.insert(event_type.to_owned());
    }
    // A stream that starts normally and then fails must surface the provider's
    // error, not fall through to the empty-content diagnostic at stream end.
    if let Some((error_type, message)) = codex_terminal_failure(value, transport) {
        return Err(ModelError::ProviderReported {
            kind: provider_reported_kind(&error_type),
            error_type,
            message,
        });
    }
    if event_type == "response.output_text.delta" {
        if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
            state.text.push_str(delta);
            if let Some(on_event) = on_event.as_mut() {
                on_event(ModelEvent::OutputDelta(delta.to_string()))?;
            }
        }
    } else if event_type.contains("reasoning") && event_type.ends_with(".delta") {
        if let Some(delta) = extract_reasoning_delta(value) {
            if !delta.is_empty() {
                state.reasoning_streamed = true;
            }
            if let Some(on_event) = on_event.as_mut() {
                if is_reasoning_summary_event(event_type) {
                    on_event(ModelEvent::ReasoningSummaryDelta(delta))?;
                } else {
                    on_event(ModelEvent::ReasoningDelta(delta))?;
                }
            }
        }
    } else if event_type == "response.output_item.added" {
        let item = value.get("item").unwrap_or(value);
        if item.get("type").and_then(|kind| kind.as_str()) == Some("function_call") {
            let index = tool_call_index(value, state);
            let arguments = item
                .get("arguments")
                .and_then(|arguments| arguments.as_str())
                .unwrap_or_default()
                .to_string();
            state
                .published_tool_arguments
                .insert(index, arguments.clone());
            if let Some(on_event) = on_event.as_mut() {
                on_event(ModelEvent::ToolCallDelta {
                    index,
                    id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(|id| id.as_str())
                        .map(str::to_string),
                    name: item
                        .get("name")
                        .and_then(|name| name.as_str())
                        .map(str::to_string),
                    arguments,
                })?;
            }
        }
    } else if event_type == "response.function_call_arguments.delta" {
        let index = tool_call_index(value, state);
        let delta = value
            .get("delta")
            .and_then(|delta| delta.as_str())
            .unwrap_or_default()
            .to_string();
        state
            .published_tool_arguments
            .entry(index)
            .or_default()
            .push_str(&delta);
        if let Some(on_event) = on_event.as_mut() {
            on_event(ModelEvent::ToolCallDelta {
                index,
                id: None,
                name: None,
                arguments: delta,
            })?;
        }
    } else if event_type == "response.function_call_arguments.done" {
        // Providers may finish a call's arguments in one restatement instead of
        // incremental deltas. Publish whatever never streamed so previews are
        // complete before the tool runs.
        let index = tool_call_index(value, state);
        let arguments = value
            .get("arguments")
            .and_then(|arguments| arguments.as_str())
            .unwrap_or_default();
        if let Some(arguments) = unpublished_tool_arguments(state, index, arguments) {
            if let Some(on_event) = on_event.as_mut() {
                on_event(ModelEvent::ToolCallDelta {
                    index,
                    id: None,
                    name: None,
                    arguments,
                })?;
            }
        }
    } else if event_type == "response.output_item.done" {
        let item = value.get("item").unwrap_or(value);
        state.output_items.push(item.clone());
        let position = value
            .get("output_index")
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok());
        if item.get("type").and_then(|value| value.as_str()) == Some("reasoning") {
            emit_output_item_replay(item.clone(), position, on_event)?;
        }
        emit_codex_search_activity(item, state, on_event)?;
        emit_image_generation_activity(item, state, on_event)?;
        persist_image_generation_item(item, position, state, on_event)?;
        if let Some(call) = extract_codex_function_call(item)? {
            // The finished item is the authoritative argument text. Identity is
            // repeated so a stream that never announced the call still reaches
            // consumers as a complete tool call.
            let index = tool_call_index(value, state);
            let arguments = item
                .get("arguments")
                .and_then(|arguments| arguments.as_str())
                .unwrap_or_default();
            if let Some(arguments) = unpublished_tool_arguments(state, index, arguments) {
                if let Some(on_event) = on_event.as_mut() {
                    on_event(ModelEvent::ToolCallDelta {
                        index,
                        id: Some(call.id.clone()),
                        name: Some(call.name.clone()),
                        arguments,
                    })?;
                }
            }
            state.tool_calls.push(call);
        }
    } else if event_type == "response.created" {
        if let Some(response_id) = value
            .get("response")
            .and_then(|response| response.get("id"))
            .or_else(|| value.get("id"))
            .and_then(|id| id.as_str())
        {
            state.response_id = Some(response_id.to_string());
        }
    } else if event_type == "response.completed" || is_steered_incomplete(value) {
        state.completed = true;
        state.steered = is_steered_incomplete(value);
        state.service_tier = value
            .get("response")
            .and_then(|response| response.get("service_tier"))
            .or_else(|| value.get("service_tier"))
            .and_then(|tier| tier.as_str())
            .map(str::to_owned);
        if state.response_id.is_none() || !state.steered {
            if let Some(response_id) = value
                .get("response")
                .and_then(|response| response.get("id"))
                .or_else(|| value.get("id"))
                .and_then(|id| id.as_str())
            {
                state.response_id = Some(response_id.to_string());
            }
        }
        state.response_status = value
            .get("response")
            .and_then(|response| response.get("status"))
            .and_then(|status| status.as_str())
            .filter(|status| !status.is_empty())
            .map(str::to_owned);
        let response = value.get("response");
        // Responses backends host reasoning models, so an aggregate total
        // without reasoning details cannot be trusted as a throughput count.
        let context = GenerationTokenContext {
            reasoning_streamed: state.reasoning_streamed,
            hidden_reasoning_risk: HiddenReasoningRisk::Possible,
        };
        let usage_report = response
            .and_then(|response| extract_usage_report(response, context))
            .or_else(|| extract_usage_report(value, context));
        if let Some(report) = usage_report {
            if let Some(on_event) = on_event.as_mut() {
                if let Some(tokens) = report.generation_output_tokens {
                    on_event(ModelEvent::GenerationOutputTokens(tokens))?;
                }
                on_event(ModelEvent::Usage(report.usage))?;
            }
        } else if state.reasoning_streamed {
            if let Some(on_event) = on_event.as_mut() {
                on_event(ModelEvent::GenerationOutputTokens(
                    rho_sdk::model::GenerationOutputTokens::Unavailable,
                ))?;
            }
        }
        // The completed envelope is authoritative, but may restate items already
        // handled by output_item.done. Reconcile each item independently so one
        // streamed output does not hide a different completed-only output.
        if let Some(output) = value
            .get("response")
            .and_then(|response| response.get("output"))
            .and_then(|output| output.as_array())
        {
            for item in output {
                emit_codex_search_activity(item, state, on_event)?;
                emit_image_generation_activity(item, state, on_event)?;
                if codex_output_item_was_processed(state, item) {
                    continue;
                }
                persist_image_generation_item(item, None, state, on_event)?;
                state.output_items.push(item.clone());
                if item.get("type").and_then(|value| value.as_str()) == Some("reasoning") {
                    emit_output_item_replay(item.clone(), None, on_event)?;
                }
                if let Some(call) = extract_codex_function_call(item)? {
                    state.tool_calls.push(call);
                }
            }
        }
        // When no text deltas streamed, recover assistant text from the completed
        // envelope even if function calls are already present. `into_response`
        // still prefers streamed text over this fallback. Leave completed_text
        // unset on empty content so empty assemblies can emit a stream-summary
        // diagnostic instead of the bare "missing response text" error.
        if state.text.is_empty() {
            if let Ok(response) =
                serde_json::from_value::<ResponsesResponse>(value["response"].clone())
            {
                if let Ok(text) = extract_response_text(response) {
                    state.completed_text = Some(text);
                }
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
#[path = "codex_sse_image_tests.rs"]
mod image_tests;

#[cfg(test)]
#[path = "codex_sse_terminal_tests.rs"]
mod terminal_tests;
