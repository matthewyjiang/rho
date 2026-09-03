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
//! With `--include-partial-messages`, Claude emits `stream_event` deltas and one
//! or more `assistant` snapshots for the same turn, ending at `message_stop`.
//! [`StreamMapper`] keeps one lifecycle per message id through stream stop:
//! deltas and ordered content-block slots own presentation for blocks they
//! emit; later snapshots and the final envelope emit only blocks not yet shown.

mod blocks;
mod events;
mod format;
mod presentation;
mod protocol;
mod tool_cards;
mod types;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use serde_json::Value;

use rho_sdk::model::ModelUsage;

use crate::{run_artifacts::AttachmentEvent, subagent::RunState};

use blocks::{emit_complete_block, emit_open_snapshot_block, note_tool_started};
use events::{decode_stream_event, ContentBlockStart, ContentDelta, StreamEventPayload};
use format::{
    context_usage_from_result, format_permission_denial, raw_usage_to_model, stringify_content,
};
// Generic effect builders and payload bounds also serve other NDJSON agent
// runtimes (see `crate::cursor_runtime::stream`). Claude-shaped mapping stays
// private to this module.
pub(crate) use format::{bound_result_text, truncate_payload_lines, MAX_TOOL_BODY_LINES};
pub(crate) use presentation::{apply_status_patch, reasoning_effects, text_effects};
use presentation::{
    clear_all_open_indexless, content_block_kind, fidelity_notice, map_error_message,
    map_rate_limit, map_system, mark_and_text, mark_slot_emitted, push_block_slot,
    reconcile_complete_block, resolve_partial_slot, set_slot_tool_id, stable_message_id,
    tool_finished_effects, tool_id_for_slot, tool_started_effects, tool_updated_effects,
    ContentBlockKind,
};
use protocol::{
    decode_line, AssistantMessage, ClaudeStreamMessage, ResultMessage, StreamEventMessage,
    SystemMessage, UserMessage,
};
use tool_cards::StartedClaudeTool;
pub(crate) use types::{
    classify_terminal_result, describe_rate_limit, notable_rate_limit_status, RateLimitInfo,
    StatusPatch, StreamEffect, TerminalClassification, TerminalResult,
};
#[cfg(test)]
pub(crate) use types::{MAX_RESULT_CHARS, MAX_TEXT_DELTA_CHARS, MAX_TOOL_PAYLOAD_CHARS};

/// Bound on concurrently tracked assistant messages.
const MAX_TRACKED_MESSAGES: usize = 64;
/// Bound on tool ids retained while a tool call is in flight.
const MAX_ACTIVE_TOOLS: usize = 256;

/// Stateful mapper for one Claude stream-json stdout session.
///
/// Reconciliation is keyed by stable message id, real content-block index, and
/// ordered content-block slots from `content_block_start`. Assistant snapshots
/// that arrive while the partial stream is still open keep the same lifecycle
/// so `StepStarted` fires once per message.
#[derive(Debug, Default)]
pub(crate) struct StreamMapper {
    /// Per-message presentation state for open / recently partial turns.
    messages: HashMap<MessageKey, MessageStreamState>,
    /// Insertion order of `messages` keys for bounded eviction.
    message_order: VecDeque<MessageKey>,
    /// Message currently receiving partial deltas, when known.
    open_message: Option<MessageKey>,
    /// Tool use ids that have started and not yet finished.
    active_tools: HashMap<String, StartedClaudeTool>,
    /// Workspace cwd from the Claude init frame, used to compact card paths.
    cwd: Option<PathBuf>,
    /// Monotonic counter for anonymous partial streams without a message id.
    anon_counter: u64,
}

/// Identity of one tracked assistant message.
///
/// Streams without a message id get their own [`Self::Anonymous`] ordinal
/// rather than sharing a synthesized string key, so distinct turns can never
/// reconcile against each other.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum MessageKey {
    /// Stable `message.id` reported by Claude.
    Stable(String),
    /// Locally allocated ordinal for a stream with no id.
    Anonymous(u64),
}

impl MessageKey {
    fn is_anonymous(&self) -> bool {
        matches!(self, Self::Anonymous(_))
    }
}

/// Presentation already emitted for one assistant message.
///
/// Ordered `block_slots` with per-slot `emitted` is the single source of truth
/// for whether a content block has been presented. `open_indexless` tracks
/// which index-less slot currently absorbs same-kind deltas.
#[derive(Debug, Default)]
struct MessageStreamState {
    step_started: bool,
    /// Every `content_block_start` in stream order, including empty bodies.
    block_slots: Vec<presentation::ContentBlockSlot>,
    /// Open index-less slot ordinals, keyed by presentation kind.
    open_indexless: presentation::OpenIndexlessSlots,
}

/// Result of allocating or resolving the open partial message.
enum OpenMessage {
    Ready {
        key: MessageKey,
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
        let message = match decode_line(line) {
            Ok(message) => message,
            Err(error) => {
                return vec![StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                    "claude stream: skipped malformed JSON line: {error}"
                )))];
            }
        };
        self.map_message(message)
    }
}

/// Stateless convenience for a single line. Prefer [`StreamMapper`] across a
/// full stream so partial and complete envelopes reconcile correctly.
#[cfg(test)]
pub(crate) fn map_line(line: &str) -> Vec<StreamEffect> {
    StreamMapper::new().push_line(line)
}

impl StreamMapper {
    fn map_message(&mut self, message: ClaudeStreamMessage) -> Vec<StreamEffect> {
        match message {
            ClaudeStreamMessage::Assistant(message) => self.map_assistant(message),
            ClaudeStreamMessage::User(message) => self.map_user_tool_result(message),
            ClaudeStreamMessage::Result(message) => self.map_result(message),
            ClaudeStreamMessage::System(message) => self.map_system_message(message),
            ClaudeStreamMessage::RateLimit(message) => map_rate_limit(message),
            ClaudeStreamMessage::StreamEvent(message) => self.map_stream_event(message),
            ClaudeStreamMessage::Error(message) => map_error_message(message),
            ClaudeStreamMessage::ProtocolControl => Vec::new(),
            ClaudeStreamMessage::Unknown { kind } => {
                vec![StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                    "claude stream: ignored unknown message type `{kind}`"
                )))]
            }
        }
    }

    fn map_assistant(&mut self, message: AssistantMessage) -> Vec<StreamEffect> {
        let message_id = stable_message_id(message.message.as_ref());
        let mut effects = Vec::new();

        // Reconcile against partial state only when we have a stable id, or
        // when a single anonymous open partial can be paired without a shared
        // fallback key that would collide across messages.
        let state_key = match message_id {
            Some(id) => Some(MessageKey::Stable(id)),
            None => self.anonymous_open_key(),
        };
        // Progressive assistant snapshots can arrive before message_stop for the
        // same open stream. Keep one lifecycle until the stream is no longer open.
        let keep_lifecycle = state_key
            .as_ref()
            .is_some_and(|key| self.open_message.as_ref() == Some(key));

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
        if !keep_lifecycle {
            if let Some(key) = &state_key {
                self.message_order.retain(|entry| entry != key);
                if self.open_message.as_ref() == Some(key) {
                    self.open_message = None;
                }
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
            self.finish_assistant_state(state_key, state, keep_lifecycle);
            return effects;
        };

        if let Some(blocks) = body.get("content").and_then(Value::as_array) {
            if keep_lifecycle {
                // Progressive snapshots can list only newly finished blocks, so
                // array indices are not stable stream indices. Reconcile by
                // slot/tool identity and never drop the open lifecycle.
                for block in blocks {
                    effects.extend(emit_open_snapshot_block(
                        &mut state,
                        block,
                        &mut self.active_tools,
                        MAX_ACTIVE_TOOLS,
                        self.cwd.as_deref(),
                    ));
                }
            } else {
                for (index, block) in blocks.iter().enumerate() {
                    let kind =
                        content_block_kind(block.get("type").and_then(Value::as_str).unwrap_or(""));
                    // Tools always go through emit so already-started cards can
                    // pick up a later complete input. Text/reasoning skip.
                    if kind != ContentBlockKind::Tool
                        && reconcile_complete_block(&mut state, kind, index)
                    {
                        continue;
                    }
                    effects.extend(emit_complete_block(
                        &mut state,
                        block,
                        index,
                        &mut self.active_tools,
                        MAX_ACTIVE_TOOLS,
                        self.cwd.as_deref(),
                    ));
                }
            }
        } else if !reconcile_complete_block(&mut state, ContentBlockKind::Text, 0) {
            // Rare shape: top-level text without a content array.
            if let Some(text) = body.get("text").and_then(Value::as_str) {
                if let Some(block_effects) = mark_and_text(&mut state, 0, text) {
                    effects.extend(block_effects);
                }
            }
        }

        self.finish_assistant_state(state_key, state, keep_lifecycle);
        effects
    }

    fn finish_assistant_state(
        &mut self,
        state_key: Option<MessageKey>,
        state: MessageStreamState,
        keep_lifecycle: bool,
    ) {
        if !keep_lifecycle {
            return;
        }
        let Some(key) = state_key else {
            return;
        };
        // `message_order` still tracks this open stream; only reinsert state.
        self.messages.insert(key, state);
    }

    fn map_system_message(&mut self, message: SystemMessage) -> Vec<StreamEffect> {
        if message.subtype.as_deref() == Some("init") {
            if let Some(cwd) = message
                .cwd
                .as_deref()
                .map(str::trim)
                .filter(|cwd| !cwd.is_empty())
            {
                self.cwd = Some(PathBuf::from(cwd));
            }
        }
        map_system(message)
    }

    fn map_user_tool_result(&mut self, message: UserMessage) -> Vec<StreamEffect> {
        let Some(body) = message.message.as_ref() else {
            return self.map_toplevel_tool_result(message);
        };
        let content = body.get("content").and_then(Value::as_array);
        let Some(blocks) = content else {
            return self.map_toplevel_tool_result(message);
        };
        let tool_blocks = blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            .collect::<Vec<_>>();
        if tool_blocks.is_empty() {
            return self.map_toplevel_tool_result(message);
        }
        let sole_result = tool_blocks.len() == 1;
        let mut effects = Vec::new();
        for block in tool_blocks {
            let ok = !block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let tool_use_id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let started = self.active_tools.remove(tool_use_id);
            let content_text = stringify_content(block.get("content"));
            effects.extend(tool_finished_effects(
                tool_use_id,
                started.as_ref(),
                ok,
                &content_text,
                enrichment_for_result(message.tool_use_result.as_ref(), tool_use_id, sole_result),
                self.cwd.as_deref(),
            ));
        }
        effects
    }

    fn map_toplevel_tool_result(&mut self, message: UserMessage) -> Vec<StreamEffect> {
        let Some(tool_use_id) = message.tool_use_id.as_deref() else {
            return Vec::new();
        };
        let started = self.active_tools.remove(tool_use_id);
        let content_text = stringify_content(message.content.as_ref());
        tool_finished_effects(
            tool_use_id,
            started.as_ref(),
            /*ok*/ true,
            &content_text,
            message.tool_use_result.as_ref(),
            self.cwd.as_deref(),
        )
    }

    fn map_result(&mut self, message: ResultMessage) -> Vec<StreamEffect> {
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

        effects.push(StreamEffect::Terminal(TerminalResult {
            classification,
            result_text,
            error,
            session_id: message.session_id,
            num_turns: message.num_turns,
            usage,
            context,
            total_cost_usd: message.total_cost_usd,
            permission_denials,
            stop_reason: message.stop_reason,
        }));
        effects
    }

    fn map_stream_event(&mut self, message: StreamEventMessage) -> Vec<StreamEffect> {
        let Some(payload) = decode_stream_event(message) else {
            return Vec::new();
        };
        match payload {
            StreamEventPayload::MessageStart { message_id } => self.map_message_start(message_id),
            StreamEventPayload::ContentBlockStart {
                message_id,
                index,
                block,
            } => self.map_content_block_start(message_id, index, block),
            StreamEventPayload::ContentBlockDelta {
                message_id,
                index,
                delta,
            } => self.map_content_block_delta(message_id, index, delta),
            StreamEventPayload::ContentBlockStop { .. } => {
                // Close any open index-less slots for this message so a later
                // same-kind index-less block allocates a fresh synthetic index.
                if let Some(key) = self.open_message.clone() {
                    if let Some(state) = self.messages.get_mut(&key) {
                        clear_all_open_indexless(state);
                    }
                }
                Vec::new()
            }
            StreamEventPayload::MessageDelta => Vec::new(),
            StreamEventPayload::MessageStop => {
                // Keep named message state until a later complete assistant
                // envelope reconciles. Drop anonymous keys here: after stop they
                // cannot pair with a later envelope and would only leak.
                if let Some(key) = self.open_message.take() {
                    if key.is_anonymous() {
                        self.messages.remove(&key);
                        self.message_order.retain(|entry| entry != &key);
                    } else if let Some(state) = self.messages.get_mut(&key) {
                        clear_all_open_indexless(state);
                    }
                }
                Vec::new()
            }
            StreamEventPayload::Unknown { kind } if !kind.is_empty() => {
                vec![StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                    "claude stream: ignored stream event `{kind}`"
                )))]
            }
            StreamEventPayload::Unknown { .. } => Vec::new(),
        }
    }

    fn map_message_start(&mut self, message_id: Option<String>) -> Vec<StreamEffect> {
        let key = match message_id {
            Some(id) => MessageKey::Stable(id),
            // No stable id: allocate a unique anonymous key so later complete
            // envelopes without ids do not share one fallback.
            None => self.next_anonymous_key(),
        };

        let mut effects = self.ensure_message_slot(&key);
        if !self.messages.contains_key(&key) {
            return effects;
        }
        self.open_message = Some(key.clone());
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
        message_id: Option<String>,
        index: Option<usize>,
        block: ContentBlockStart,
    ) -> Vec<StreamEffect> {
        let (message_key, mut effects) = match self.ensure_open_message(message_id) {
            OpenMessage::Ready { key, notices } => (key, notices),
            OpenMessage::Unavailable { notices } => {
                let mut effects = notices;
                effects.extend(fidelity_notice(
                    "claude stream: dropped content_block_start; could not allocate message state",
                ));
                return effects;
            }
        };
        let kind = block.kind();

        {
            let Some(state) = self.messages.get_mut(&message_key) else {
                return effects;
            };
            // Track every start, including empty bodies, so complete envelopes
            // reconcile against the exact ordered slots Claude opened.
            if push_block_slot(state, kind, index).is_none() {
                effects.extend(fidelity_notice(
                    "claude stream: dropped content_block_start; tracked block cap reached",
                ));
                return effects;
            }
        }

        match block {
            ContentBlockStart::ToolUse {
                ref id,
                ref name,
                ref input,
            } => {
                let tool_id = id.clone().unwrap_or_default();
                let already_started =
                    !tool_id.is_empty() && self.active_tools.contains_key(&tool_id);
                match self.claim_partial_slot(&message_key, index, kind, "partial tool start") {
                    // Already started: the slot claim above keeps ordering, but
                    // the tool must not be announced twice.
                    Ok(ordinal) if already_started => {
                        if let Some(state) = self.messages.get_mut(&message_key) {
                            set_slot_tool_id(state, ordinal, &tool_id);
                        }
                        if let Some(tool) = self.active_tools.get_mut(&tool_id) {
                            if tool.apply_input(input.as_ref()) {
                                let tool = tool.clone();
                                effects.extend(tool_updated_effects(
                                    &tool_id,
                                    &tool,
                                    self.cwd.as_deref(),
                                ));
                            }
                        }
                        effects
                    }
                    Ok(ordinal) => {
                        let tool =
                            StartedClaudeTool::from_name_input(name.as_deref(), input.as_ref());
                        if let Some(state) = self.messages.get_mut(&message_key) {
                            set_slot_tool_id(state, ordinal, &tool_id);
                        }
                        if let Some(notice) = note_tool_started(
                            &mut self.active_tools,
                            MAX_ACTIVE_TOOLS,
                            &tool_id,
                            tool.clone(),
                        ) {
                            effects.extend(notice);
                        }
                        effects.extend(tool_started_effects(&tool_id, &tool, self.cwd.as_deref()));
                        effects
                    }
                    // A restarted tool never needed presentation, so a cap here
                    // costs nothing worth reporting.
                    Err(_) if already_started => effects,
                    Err(notices) => {
                        effects.extend(notices);
                        effects
                    }
                }
            }
            ContentBlockStart::Text { text } => {
                effects.extend(self.emit_partial_text(
                    &message_key,
                    index,
                    kind,
                    "partial text",
                    &text,
                    text_effects,
                ));
                effects
            }
            ContentBlockStart::Thinking { text } => {
                effects.extend(self.emit_partial_text(
                    &message_key,
                    index,
                    kind,
                    "partial reasoning",
                    &text,
                    reasoning_effects,
                ));
                effects
            }
            ContentBlockStart::Other => effects,
        }
    }

    /// Claim the ordered slot a partial event belongs to.
    ///
    /// `Err` carries the fidelity notice for a block that cannot be tracked, so
    /// dropped presentation stays visible in the transcript. Missing message
    /// state is silent: the caller already reported that.
    fn claim_partial_slot(
        &mut self,
        key: &MessageKey,
        index: Option<usize>,
        kind: ContentBlockKind,
        what: &str,
    ) -> Result<usize, Vec<StreamEffect>> {
        let Some(state) = self.messages.get_mut(key) else {
            return Err(Vec::new());
        };
        let dropped = || {
            Err(fidelity_notice(&format!(
                "claude stream: dropped {what}; tracked block cap reached"
            )))
        };
        let Some(ordinal) = resolve_partial_slot(state, index, kind) else {
            return dropped();
        };
        if !mark_slot_emitted(state, ordinal, index) {
            return dropped();
        }
        Ok(ordinal)
    }

    /// Claim a slot and present `text` through `present`.
    ///
    /// Empty bodies emit nothing: a start with no text only reserves its slot.
    fn emit_partial_text(
        &mut self,
        key: &MessageKey,
        index: Option<usize>,
        kind: ContentBlockKind,
        what: &str,
        text: &str,
        present: fn(&str) -> Vec<StreamEffect>,
    ) -> Vec<StreamEffect> {
        if text.is_empty() {
            return Vec::new();
        }
        match self.claim_partial_slot(key, index, kind, what) {
            Ok(_) => present(text),
            Err(notices) => notices,
        }
    }

    fn apply_input_json_delta(
        &mut self,
        key: &MessageKey,
        index: Option<usize>,
        partial_json: &str,
    ) -> Vec<StreamEffect> {
        if partial_json.is_empty() {
            return Vec::new();
        }
        let Some(state) = self.messages.get(key) else {
            return Vec::new();
        };
        let Some(tool_id) = tool_id_for_slot(state, index) else {
            return Vec::new();
        };
        let Some(tool) = self.active_tools.get_mut(&tool_id) else {
            return Vec::new();
        };
        if !tool.push_input_json(partial_json) {
            return Vec::new();
        }
        let tool = tool.clone();
        tool_updated_effects(&tool_id, &tool, self.cwd.as_deref())
    }

    fn map_content_block_delta(
        &mut self,
        message_id: Option<String>,
        index: Option<usize>,
        delta: ContentDelta,
    ) -> Vec<StreamEffect> {
        let (message_key, mut effects) = match self.ensure_open_message(message_id) {
            OpenMessage::Ready { key, notices } => (key, notices),
            OpenMessage::Unavailable { notices } => {
                let mut effects = notices;
                effects.extend(fidelity_notice(
                    "claude stream: dropped content_block_delta; could not allocate message state",
                ));
                return effects;
            }
        };

        match delta {
            ContentDelta::Text { text } => {
                effects.extend(self.emit_partial_text(
                    &message_key,
                    index,
                    ContentBlockKind::Text,
                    "text delta",
                    &text,
                    text_effects,
                ));
                effects
            }
            ContentDelta::Thinking { text } => {
                effects.extend(self.emit_partial_text(
                    &message_key,
                    index,
                    ContentBlockKind::Reasoning,
                    "reasoning delta",
                    &text,
                    reasoning_effects,
                ));
                effects
            }
            ContentDelta::InputJson { partial_json } => {
                effects.extend(self.apply_input_json_delta(&message_key, index, &partial_json));
                effects
            }
            ContentDelta::Signature => effects,
            ContentDelta::Other { type_name } if !type_name.is_empty() => {
                effects.push(StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                    "claude stream: ignored delta `{type_name}`"
                ))));
                effects
            }
            ContentDelta::Other { .. } => effects,
        }
    }

    /// Ensure there is an open message key for partial events.
    ///
    /// Partial events without a prior `message_start` and without an embedded
    /// message id get a unique anonymous key. They never share a global
    /// fallback that would incorrectly reconcile distinct turns.
    fn ensure_open_message(&mut self, message_id: Option<String>) -> OpenMessage {
        if let Some(key) = self.open_message.clone() {
            if self.messages.contains_key(&key) {
                return OpenMessage::Ready {
                    key,
                    notices: Vec::new(),
                };
            }
        }
        let key = match message_id {
            Some(id) => MessageKey::Stable(id),
            None => self.next_anonymous_key(),
        };
        let notices = self.ensure_message_slot(&key);
        if !self.messages.contains_key(&key) {
            return OpenMessage::Unavailable { notices };
        }
        self.open_message = Some(key.clone());
        OpenMessage::Ready { key, notices }
    }

    fn ensure_message_slot(&mut self, key: &MessageKey) -> Vec<StreamEffect> {
        if self.messages.contains_key(key) {
            return Vec::new();
        }
        let mut notices = Vec::new();
        while self.messages.len() >= MAX_TRACKED_MESSAGES {
            let Some(old) = self.message_order.pop_front() else {
                break;
            };
            if &old == key {
                continue;
            }
            if self.messages.remove(&old).is_some() {
                if self.open_message.as_ref() == Some(&old) {
                    self.open_message = None;
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
            .insert(key.clone(), MessageStreamState::default());
        self.message_order.push_back(key.clone());
        notices
    }

    fn next_anonymous_key(&mut self) -> MessageKey {
        self.anon_counter = self.anon_counter.saturating_add(1);
        MessageKey::Anonymous(self.anon_counter)
    }

    /// The open stream's key when it has no stable id, so a complete envelope
    /// without an id can still pair with exactly one anonymous stream.
    fn anonymous_open_key(&self) -> Option<MessageKey> {
        self.open_message.clone().filter(MessageKey::is_anonymous)
    }
}

/// Pair a Claude `tool_use_result` sibling with one `tool_result` block.
///
/// A lone result takes the sibling as-is. Batched results only keep
/// enrichment that names the same `tool_use_id`, so a Read file object is
/// never applied to a parallel Edit.
fn enrichment_for_result<'a>(
    tool_use_result: Option<&'a Value>,
    tool_use_id: &str,
    sole_result: bool,
) -> Option<&'a Value> {
    let value = tool_use_result?;
    if sole_result {
        return Some(value);
    }
    match value {
        Value::Array(items) => items
            .iter()
            .find(|item| item.get("tool_use_id").and_then(Value::as_str) == Some(tool_use_id)),
        Value::Object(_)
            if value.get("tool_use_id").and_then(Value::as_str) == Some(tool_use_id) =>
        {
            Some(value)
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "stream_test_support.rs"]
mod stream_test_support;

#[cfg(test)]
#[path = "stream_protocol_tests.rs"]
mod stream_protocol_tests;

#[cfg(test)]
#[path = "stream_reconciliation_tests.rs"]
mod stream_reconciliation_tests;
