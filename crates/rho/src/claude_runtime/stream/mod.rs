//! Decode Claude Code `stream-json` messages into Rho run artifacts.
//!
//! The schema is unstable. Known variants are typed; unknown messages become
//! notices and never fail the run.
//!
//! # Terminal results and process exit
//!
//! A parsed `result` message is emitted as [`StreamEffect::Terminal`] with an
//! explicit [`TerminalClassification`]. Mapping is metadata-only for that
//! message: it never publishes [`RunState::Ok`] / [`RunState::Error`] and never
//! emits [`AttachmentEvent::Completed`] / [`AttachmentEvent::Failed`]. Protocol
//! `type:error` is likewise pending metadata (error text + failure terminal),
//! not a terminal run state or Failed attachment. The session caller combines
//! the classification with the child exit status and writes exactly one
//! terminal attachment.
//!
//! # Partial messages
//!
//! With `--include-partial-messages`, Claude emits `stream_event` deltas and a
//! complete `assistant` envelope for the same turn. [`StreamMapper`] reconciles
//! per message id and content-block index: deltas own presentation for blocks
//! they emit; the complete envelope emits only blocks that were not streamed.

mod format;
mod presentation;
mod types;

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Deserialize;
use serde_json::Value;

use rho_sdk::model::ModelUsage;
use rho_tools::tool::ToolDisplayStyle;

use crate::{subagent::RunState, tui::AttachmentEvent};

use format::{
    bound_result_text, context_usage_from_result, format_permission_denial, raw_usage_to_model,
    stringify_content, RawUsage,
};
pub(crate) use presentation::apply_status_patch;
use presentation::{
    block_index, fidelity_notice, map_error_message, map_rate_limit, map_system,
    mark_and_reasoning, mark_and_text, reasoning_effects, record_block, stable_message_id,
    text_effects, tool_finished_effects, tool_started_effects,
};
pub(crate) use types::{
    classify_terminal_result, describe_rate_limit, RateLimitInfo, StatusPatch, StreamEffect,
    TerminalClassification, TerminalResult,
};
#[cfg(test)]
pub(crate) use types::{MAX_RESULT_CHARS, MAX_TEXT_DELTA_CHARS, MAX_TOOL_PAYLOAD_CHARS};

/// Claude tools are rendered with the generic default style. Claude tool names
/// do not map onto Rho's file/diff/web display kinds.
pub(crate) const CLAUDE_TOOL_DISPLAY_STYLE: ToolDisplayStyle = ToolDisplayStyle::DefaultTool;

/// Bound on concurrently tracked assistant messages.
const MAX_TRACKED_MESSAGES: usize = 64;
/// Bound on tool ids retained while a tool call is in flight.
const MAX_ACTIVE_TOOLS: usize = 256;

/// Stateful mapper for one Claude stream-json stdout session.
///
/// Reconciliation is keyed by stable message id + content-block index. Only
/// blocks that actually emitted presentation are recorded, so a complete
/// assistant envelope can still surface blocks that never streamed.
#[derive(Debug, Default)]
pub(crate) struct StreamMapper {
    /// Per-message presentation state for open / recently partial turns.
    messages: HashMap<String, MessageStreamState>,
    /// Insertion order of `messages` keys for bounded eviction.
    message_order: VecDeque<String>,
    /// Message currently receiving partial deltas, when known.
    open_message_id: Option<String>,
    /// Tool use ids that have started and not yet finished.
    active_tools: HashSet<String>,
    /// Monotonic counter for anonymous partial streams without a message id.
    anon_counter: u64,
}

/// Presentation already emitted for one assistant message.
#[derive(Debug, Default)]
struct MessageStreamState {
    step_started: bool,
    /// Content-block indices that produced text, reasoning, or tool-start output.
    emitted_blocks: HashSet<usize>,
}

/// Result of allocating or resolving the open partial message.
enum OpenMessage {
    Ready {
        key: String,
        notices: Vec<StreamEffect>,
    },
    Unavailable {
        notices: Vec<StreamEffect>,
    },
}

impl StreamMapper {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Map one NDJSON line. Empty lines are ignored. Malformed JSON becomes a
    /// notice; the run continues.
    pub(crate) fn push_line(&mut self, line: &str) -> Vec<StreamEffect> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let envelope = match serde_json::from_str::<StreamEnvelope>(line) {
            Ok(envelope) => envelope,
            Err(error) => {
                return vec![StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                    "claude stream: skipped malformed JSON line: {error}"
                )))];
            }
        };
        self.map_envelope(envelope)
    }
}

/// Stateless convenience for a single line. Prefer [`StreamMapper`] across a
/// full stream so partial and complete envelopes reconcile correctly.
#[cfg(test)]
pub(crate) fn map_line(line: &str) -> Vec<StreamEffect> {
    StreamMapper::new().push_line(line)
}

/// Top-level stream-json `type` policy.
///
/// Protocol/control frames are known and intentionally silent so heartbeats do
/// not spam attach notices. Unknown kinds remain visible diagnostics.
#[derive(Debug, PartialEq, Eq)]
enum TopLevelKind {
    Assistant,
    User,
    Result,
    System,
    RateLimitEvent,
    StreamEvent,
    Error,
    /// Documented no-op frames: progress/status heartbeats and control channel.
    ProtocolControlNoop,
    Unknown(String),
}

fn classify_top_level_kind(kind: &str) -> TopLevelKind {
    match kind {
        "assistant" => TopLevelKind::Assistant,
        "user" => TopLevelKind::User,
        "result" => TopLevelKind::Result,
        "system" => TopLevelKind::System,
        "rate_limit_event" => TopLevelKind::RateLimitEvent,
        "stream_event" => TopLevelKind::StreamEvent,
        "error" => TopLevelKind::Error,
        // Intentional silence: these are protocol/control or progress heartbeats.
        "tool_progress" | "status" | "keep_alive" | "control_request" | "control_response" => {
            TopLevelKind::ProtocolControlNoop
        }
        other => TopLevelKind::Unknown(other.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct StreamEnvelope {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    num_turns: Option<u64>,
    #[serde(default)]
    total_cost_usd: Option<f64>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<RawUsage>,
    #[serde(default, rename = "modelUsage")]
    model_usage: Option<Value>,
    #[serde(default)]
    permission_denials: Option<Vec<Value>>,
    #[serde(default)]
    message: Option<Value>,
    #[serde(default)]
    event: Option<Value>,
    #[serde(default)]
    rate_limit_info: Option<RateLimitInfo>,
    #[serde(default)]
    content_block: Option<Value>,
    #[serde(default)]
    delta: Option<Value>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    content: Option<Value>,
    #[serde(flatten)]
    _rest: Value,
}

impl StreamMapper {
    fn map_envelope(&mut self, message: StreamEnvelope) -> Vec<StreamEffect> {
        // Exhaustive top-level policy: known frames are handled, documented
        // protocol/control frames are intentional no-ops (no heartbeat spam),
        // and unknown kinds become diagnostics so schema drift is visible.
        match classify_top_level_kind(message.kind.as_str()) {
            TopLevelKind::Assistant => self.map_assistant(message),
            TopLevelKind::User => self.map_user_tool_result(message),
            TopLevelKind::Result => self.map_result(message),
            TopLevelKind::System => map_system(message),
            TopLevelKind::RateLimitEvent => map_rate_limit(message),
            TopLevelKind::StreamEvent => self.map_stream_event(message),
            TopLevelKind::Error => map_error_message(message),
            TopLevelKind::ProtocolControlNoop => Vec::new(),
            TopLevelKind::Unknown(other) => {
                vec![StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                    "claude stream: ignored unknown message type `{other}`"
                )))]
            }
        }
    }

    fn map_assistant(&mut self, message: StreamEnvelope) -> Vec<StreamEffect> {
        let message_id = stable_message_id(message.message.as_ref());
        let mut effects = Vec::new();

        // Reconcile against partial state only when we have a stable id, or
        // when a single anonymous open partial can be paired without a shared
        // fallback key that would collide across messages.
        let state_key = match message_id {
            Some(id) => Some(id),
            None => self.take_anonymous_open_key(),
        };

        if let Some(session_id) = message.session_id.clone() {
            effects.push(StreamEffect::Status(StatusPatch {
                claude_session_id: Some(session_id),
                ..StatusPatch::default()
            }));
        }

        let mut state = state_key
            .as_ref()
            .and_then(|key| self.messages.remove(key))
            .unwrap_or_default();
        if let Some(key) = &state_key {
            self.message_order.retain(|entry| entry != key);
            if self.open_message_id.as_ref() == Some(key) {
                self.open_message_id = None;
            }
        }

        if !state.step_started {
            state.step_started = true;
            effects.push(StreamEffect::Attachment(AttachmentEvent::StepStarted));
        }
        effects.push(StreamEffect::Status(StatusPatch {
            state: Some(RunState::Running),
            last_activity: Some("assistant".into()),
            ..StatusPatch::default()
        }));

        let Some(body) = message.message.as_ref() else {
            // Complete envelope consumed; drop state.
            return effects;
        };

        if let Some(blocks) = body.get("content").and_then(Value::as_array) {
            for (index, block) in blocks.iter().enumerate() {
                if state.emitted_blocks.contains(&index) {
                    continue;
                }
                effects.extend(self.emit_complete_block(block, index, &mut state));
            }
        } else if !state.emitted_blocks.contains(&0) {
            // Rare shape: top-level text without a content array.
            if let Some(text) = body.get("text").and_then(Value::as_str) {
                if let Some(block_effects) = mark_and_text(&mut state, 0, text) {
                    effects.extend(block_effects);
                }
            }
        }

        // Message is complete: do not retain its block set.
        effects
    }

    fn emit_complete_block(
        &mut self,
        block: &Value,
        index: usize,
        state: &mut MessageStreamState,
    ) -> Vec<StreamEffect> {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "text" => {
                let Some(text) = block.get("text").and_then(Value::as_str) else {
                    return Vec::new();
                };
                mark_and_text(state, index, text).unwrap_or_else(|| {
                    fidelity_notice(
                        "claude stream: dropped complete text block; tracked block cap reached",
                    )
                })
            }
            "thinking" => {
                let Some(text) = block
                    .get("thinking")
                    .or_else(|| block.get("text"))
                    .and_then(Value::as_str)
                else {
                    return Vec::new();
                };
                mark_and_reasoning(state, index, text).unwrap_or_else(|| {
                    fidelity_notice(
                        "claude stream: dropped complete reasoning block; tracked block cap reached",
                    )
                })
            }
            "tool_use" => {
                let tool_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !tool_id.is_empty() && self.active_tools.contains(&tool_id) {
                    // Started via partials; still mark this complete index seen.
                    let _ = record_block(state, index);
                    return Vec::new();
                }
                if !record_block(state, index) {
                    return fidelity_notice(
                        "claude stream: dropped complete tool block; tracked block cap reached",
                    );
                }
                let mut effects = Vec::new();
                if let Some(notice) = self.note_tool_started(&tool_id) {
                    effects.extend(notice);
                }
                effects.extend(tool_started_effects(block));
                effects
            }
            other if !other.is_empty() => {
                vec![StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                    "claude stream: ignored assistant block `{other}`"
                )))]
            }
            _ => Vec::new(),
        }
    }

    fn map_user_tool_result(&mut self, message: StreamEnvelope) -> Vec<StreamEffect> {
        let Some(body) = message.message.as_ref() else {
            return self.map_toplevel_tool_result(message);
        };
        let content = body.get("content").and_then(Value::as_array);
        let Some(blocks) = content else {
            return self.map_toplevel_tool_result(message);
        };
        let mut effects = Vec::new();
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let ok = !block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let tool_use_id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            self.active_tools.remove(tool_use_id);
            let content_text = stringify_content(block.get("content"));
            effects.extend(tool_finished_effects(tool_use_id, ok, &content_text));
        }
        if effects.is_empty() {
            return self.map_toplevel_tool_result(message);
        }
        effects
    }

    fn map_toplevel_tool_result(&mut self, message: StreamEnvelope) -> Vec<StreamEffect> {
        let Some(tool_use_id) = message.tool_use_id.as_deref() else {
            return Vec::new();
        };
        self.active_tools.remove(tool_use_id);
        let content_text = stringify_content(message.content.as_ref());
        tool_finished_effects(tool_use_id, /*ok*/ true, &content_text)
    }

    fn map_result(&mut self, message: StreamEnvelope) -> Vec<StreamEffect> {
        let classification = classify_terminal_result(message.subtype.as_deref(), message.is_error);
        let usage = message.usage.as_ref().map(raw_usage_to_model);
        // RunStatus.input_tokens and ContextUsage both use uncached + cache-read
        // + cache-creation so attach metrics stay aligned.
        let input_tokens = usage.as_ref().and_then(ModelUsage::total_input_tokens);
        let output_tokens = usage.as_ref().and_then(|usage| usage.output_tokens);
        let context = context_usage_from_result(message.model_usage.as_ref(), usage.as_ref());
        let permission_denials = message
            .permission_denials
            .unwrap_or_default()
            .into_iter()
            .filter_map(format_permission_denial)
            .collect::<Vec<_>>();

        let mut effects = Vec::new();
        for denial in &permission_denials {
            effects.push(StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                "claude permission denied: {denial}"
            ))));
        }
        if let Some(usage) = usage.clone() {
            effects.push(StreamEffect::Attachment(AttachmentEvent::Usage(usage)));
        }
        if let Some(context) = context.clone() {
            effects.push(StreamEffect::Attachment(AttachmentEvent::ContextUsage(
                context,
            )));
        }

        let result_text = message.result.as_deref().map(bound_result_text);
        let error = match &classification {
            TerminalClassification::Success { .. } => None,
            TerminalClassification::Failure { subtype, .. } => Some(
                result_text
                    .clone()
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| format!("claude result subtype: {subtype}")),
            ),
            TerminalClassification::Invalid { reason } => Some(reason.clone()),
        };

        // Metadata only. Do not publish RunState::Ok/Error or terminal
        // Completed/Failed attachments here; session owns those after exit.
        effects.push(StreamEffect::Status(StatusPatch {
            turns: message.num_turns,
            input_tokens,
            output_tokens,
            result: result_text.clone(),
            error: error.clone(),
            claude_session_id: message.session_id.clone(),
            total_cost_usd: message.total_cost_usd,
            last_activity: Some(match &classification {
                TerminalClassification::Success { .. } => "result received".into(),
                TerminalClassification::Failure { .. } => "result failed".into(),
                TerminalClassification::Invalid { .. } => "result invalid".into(),
            }),
            ..StatusPatch::default()
        }));

        let ok = classification.is_success();
        effects.push(StreamEffect::Terminal(TerminalResult {
            classification,
            ok,
            result_text,
            error,
            session_id: message.session_id,
            num_turns: message.num_turns,
            usage,
            context,
            total_cost_usd: message.total_cost_usd,
            permission_denials,
            stop_reason: message.stop_reason,
            subtype: message.subtype,
            is_error: message.is_error,
        }));
        effects
    }

    fn map_stream_event(&mut self, message: StreamEnvelope) -> Vec<StreamEffect> {
        let Some(event) = message.event else {
            return Vec::new();
        };
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "message_start" => self.map_message_start(&event),
            "content_block_start" => {
                self.map_content_block_start(&event, message.content_block.as_ref())
            }
            "content_block_delta" => self.map_content_block_delta(&event, message.delta.as_ref()),
            "content_block_stop" | "message_delta" => Vec::new(),
            "message_stop" => {
                // Keep message state until the complete assistant envelope
                // reconciles; only clear the open cursor.
                self.open_message_id = None;
                Vec::new()
            }
            other if !other.is_empty() => vec![StreamEffect::Attachment(AttachmentEvent::Notice(
                format!("claude stream: ignored stream event `{other}`"),
            ))],
            _ => Vec::new(),
        }
    }

    fn map_message_start(&mut self, event: &Value) -> Vec<StreamEffect> {
        let key = match event
            .get("message")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            Some(id) => id,
            None => {
                // No stable id: allocate a unique anonymous key so later
                // complete envelopes without ids do not share one fallback.
                self.anon_counter = self.anon_counter.saturating_add(1);
                format!("anon:{}", self.anon_counter)
            }
        };

        let mut effects = self.ensure_message_slot(&key);
        if !self.messages.contains_key(&key) {
            return effects;
        }
        self.open_message_id = Some(key.clone());
        let state = self.messages.get_mut(&key).expect("message slot present");
        if !state.step_started {
            state.step_started = true;
            effects.push(StreamEffect::Attachment(AttachmentEvent::StepStarted));
            effects.push(StreamEffect::Status(StatusPatch {
                state: Some(RunState::Running),
                last_activity: Some("assistant".into()),
                ..StatusPatch::default()
            }));
        }
        effects
    }

    fn map_content_block_start(
        &mut self,
        event: &Value,
        top_level_block: Option<&Value>,
    ) -> Vec<StreamEffect> {
        let (message_key, mut effects) = match self.ensure_open_message(event) {
            OpenMessage::Ready { key, notices } => (key, notices),
            OpenMessage::Unavailable { notices } => {
                let mut effects = notices;
                effects.extend(fidelity_notice(
                    "claude stream: dropped content_block_start; could not allocate message state",
                ));
                return effects;
            }
        };
        let index = block_index(event);
        let block = event
            .get("content_block")
            .or(top_level_block)
            .cloned()
            .unwrap_or(Value::Null);

        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                let tool_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !tool_id.is_empty() && self.active_tools.contains(&tool_id) {
                    return effects;
                }
                if let Some(index) = index {
                    let Some(state) = self.messages.get_mut(&message_key) else {
                        return effects;
                    };
                    if state.emitted_blocks.contains(&index) {
                        return effects;
                    }
                    if !record_block(state, index) {
                        effects.extend(fidelity_notice(
                            "claude stream: dropped partial tool start; tracked block cap reached",
                        ));
                        return effects;
                    }
                }
                if let Some(notice) = self.note_tool_started(&tool_id) {
                    effects.extend(notice);
                }
                effects.extend(tool_started_effects(&block));
                effects
            }
            Some("text") => {
                // Opening a text block does not count as emission until content
                // arrives (delta or non-empty initial text).
                let Some(text) = block.get("text").and_then(Value::as_str) else {
                    return effects;
                };
                if text.is_empty() {
                    return effects;
                }
                if let Some(index) = index {
                    let Some(state) = self.messages.get_mut(&message_key) else {
                        return effects;
                    };
                    match mark_and_text(state, index, text) {
                        Some(block_effects) => effects.extend(block_effects),
                        None => effects.extend(fidelity_notice(
                            "claude stream: dropped partial text; tracked block cap reached",
                        )),
                    }
                    return effects;
                }
                effects.extend(text_effects(text));
                effects
            }
            Some("thinking") => {
                let Some(text) = block
                    .get("thinking")
                    .or_else(|| block.get("text"))
                    .and_then(Value::as_str)
                else {
                    return effects;
                };
                if text.is_empty() {
                    return effects;
                }
                if let Some(index) = index {
                    let Some(state) = self.messages.get_mut(&message_key) else {
                        return effects;
                    };
                    match mark_and_reasoning(state, index, text) {
                        Some(block_effects) => effects.extend(block_effects),
                        None => effects.extend(fidelity_notice(
                            "claude stream: dropped partial reasoning; tracked block cap reached",
                        )),
                    }
                    return effects;
                }
                effects.extend(reasoning_effects(text));
                effects
            }
            _ => effects,
        }
    }

    fn map_content_block_delta(
        &mut self,
        event: &Value,
        top_level_delta: Option<&Value>,
    ) -> Vec<StreamEffect> {
        let (message_key, mut effects) = match self.ensure_open_message(event) {
            OpenMessage::Ready { key, notices } => (key, notices),
            OpenMessage::Unavailable { notices } => {
                let mut effects = notices;
                effects.extend(fidelity_notice(
                    "claude stream: dropped content_block_delta; could not allocate message state",
                ));
                return effects;
            }
        };
        let index = block_index(event);
        let Some(delta) = event.get("delta").or(top_level_delta) else {
            return effects;
        };
        let delta_type = delta.get("type").and_then(Value::as_str).unwrap_or("");

        match delta_type {
            "text_delta" => {
                let Some(text) = delta.get("text").and_then(Value::as_str) else {
                    return effects;
                };
                if text.is_empty() {
                    return effects;
                }
                if let Some(index) = index {
                    let Some(state) = self.messages.get_mut(&message_key) else {
                        return effects;
                    };
                    // First delta marks the block as streamed; later deltas still emit.
                    if !record_block(state, index) {
                        effects.extend(fidelity_notice(
                            "claude stream: dropped text delta; tracked block cap reached",
                        ));
                        return effects;
                    }
                }
                effects.extend(text_effects(text));
                effects
            }
            "thinking_delta" | "reasoning_delta" => {
                let Some(text) = delta
                    .get("thinking")
                    .or_else(|| delta.get("text"))
                    .and_then(Value::as_str)
                else {
                    return effects;
                };
                if text.is_empty() {
                    return effects;
                }
                if let Some(index) = index {
                    let Some(state) = self.messages.get_mut(&message_key) else {
                        return effects;
                    };
                    if !record_block(state, index) {
                        effects.extend(fidelity_notice(
                            "claude stream: dropped reasoning delta; tracked block cap reached",
                        ));
                        return effects;
                    }
                }
                effects.extend(reasoning_effects(text));
                effects
            }
            "input_json_delta" => effects,
            other if !other.is_empty() => {
                effects.push(StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                    "claude stream: ignored delta `{other}`"
                ))));
                effects
            }
            _ => effects,
        }
    }

    /// Ensure there is an open message key for partial events.
    ///
    /// Partial events without a prior `message_start` and without an embedded
    /// message id get a unique anonymous key. They never share a global
    /// fallback that would incorrectly reconcile distinct turns.
    fn ensure_open_message(&mut self, event: &Value) -> OpenMessage {
        if let Some(key) = self.open_message_id.clone() {
            if self.messages.contains_key(&key) {
                return OpenMessage::Ready {
                    key,
                    notices: Vec::new(),
                };
            }
        }
        let key = event
            .get("message")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                self.anon_counter = self.anon_counter.saturating_add(1);
                format!("anon:{}", self.anon_counter)
            });
        let notices = self.ensure_message_slot(&key);
        if !self.messages.contains_key(&key) {
            return OpenMessage::Unavailable { notices };
        }
        self.open_message_id = Some(key.clone());
        OpenMessage::Ready { key, notices }
    }

    fn ensure_message_slot(&mut self, key: &str) -> Vec<StreamEffect> {
        if self.messages.contains_key(key) {
            return Vec::new();
        }
        let mut notices = Vec::new();
        while self.messages.len() >= MAX_TRACKED_MESSAGES {
            let Some(old) = self.message_order.pop_front() else {
                break;
            };
            if old == key {
                continue;
            }
            if self.messages.remove(&old).is_some() {
                if self.open_message_id.as_deref() == Some(old.as_str()) {
                    self.open_message_id = None;
                }
                notices.push(StreamEffect::Attachment(AttachmentEvent::Notice(
                    "claude stream: evicted tracked message state; later complete envelopes may duplicate or omit presentation".into(),
                )));
            }
        }
        if self.messages.len() >= MAX_TRACKED_MESSAGES {
            notices.extend(fidelity_notice(
                "claude stream: cannot track message; tracked message cap reached",
            ));
            return notices;
        }
        self.messages
            .insert(key.to_string(), MessageStreamState::default());
        self.message_order.push_back(key.to_string());
        notices
    }

    fn take_anonymous_open_key(&mut self) -> Option<String> {
        let key = self.open_message_id.clone()?;
        if key.starts_with("anon:") {
            self.open_message_id = None;
            Some(key)
        } else {
            None
        }
    }

    fn note_tool_started(&mut self, tool_id: &str) -> Option<Vec<StreamEffect>> {
        if tool_id.is_empty() {
            return None;
        }
        if self.active_tools.contains(tool_id) {
            return None;
        }
        if self.active_tools.len() >= MAX_ACTIVE_TOOLS {
            // Drop an arbitrary active id so pathological streams stay bounded.
            if let Some(old) = self.active_tools.iter().next().cloned() {
                self.active_tools.remove(&old);
                self.active_tools.insert(tool_id.to_string());
                return Some(fidelity_notice(
                    "claude stream: evicted active tool id; tool finish pairing may be imperfect",
                ));
            }
        }
        self.active_tools.insert(tool_id.to_string());
        None
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
