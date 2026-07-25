//! Presentation helpers shared by the Claude stream mapper.

use serde_json::Value;

use crate::{
    run_artifacts::AttachmentEvent,
    subagent::{RunState, RunStatus},
};

use super::format::{
    append_tail, bound_delta_text, bound_text, stringify_content, truncate_payload_lines,
    LAST_TEXT_BYTES, MAX_TOOL_DISPLAY_LINES,
};
use super::protocol::{ErrorMessage, RateLimitMessage, SystemMessage};
use super::types::{
    StatusPatch, StreamEffect, TerminalClassification, TerminalResult, MAX_RESULT_CHARS,
};
use super::{describe_rate_limit, MessageStreamState, CLAUDE_TOOL_DISPLAY_STYLE};

/// Bound on content-block indices recorded per message.
pub(super) const MAX_BLOCKS_PER_MESSAGE: usize = 256;

pub(super) fn stable_message_id(message: Option<&Value>) -> Option<String> {
    message
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// Presentation kind for an ordered content-block slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContentBlockKind {
    Text,
    Reasoning,
    Tool,
    Other,
}

impl ContentBlockKind {
    /// Slot index in [`OpenIndexlessSlots`]. `Other` is never tracked.
    fn indexless_position(self) -> Option<usize> {
        match self {
            Self::Text => Some(0),
            Self::Reasoning => Some(1),
            Self::Tool => Some(2),
            Self::Other => None,
        }
    }
}

/// Open index-less slot ordinal per tracked kind.
///
/// Partial events may omit `index`; the open slot for that kind absorbs later
/// deltas until a `content_block_stop` (or an indexed start) closes it.
#[derive(Debug, Default)]
pub(super) struct OpenIndexlessSlots([Option<usize>; 3]);

impl OpenIndexlessSlots {
    fn get(&self, kind: ContentBlockKind) -> Option<usize> {
        self.0[kind.indexless_position()?]
    }

    fn open(&mut self, kind: ContentBlockKind, ordinal: usize) {
        if let Some(position) = kind.indexless_position() {
            self.0[position] = Some(ordinal);
        }
    }

    fn close(&mut self, kind: ContentBlockKind) {
        if let Some(position) = kind.indexless_position() {
            self.0[position] = None;
        }
    }

    fn close_all(&mut self) {
        self.0 = [None; 3];
    }
}

/// One content block opened by a partial `content_block_start`.
#[derive(Debug, Clone, Copy)]
pub(super) struct ContentBlockSlot {
    pub(super) kind: ContentBlockKind,
    /// Explicit stream index when Claude provided one.
    pub(super) index: Option<usize>,
    /// True once this slot produced text, reasoning, or tool-start output.
    pub(super) emitted: bool,
}

pub(super) fn content_block_kind(type_name: &str) -> ContentBlockKind {
    match type_name {
        "text" => ContentBlockKind::Text,
        "thinking" => ContentBlockKind::Reasoning,
        "tool_use" => ContentBlockKind::Tool,
        _ => ContentBlockKind::Other,
    }
}

/// Record a `content_block_start` in stream order, including empty bodies.
///
/// Starts always append so index-less and indexed blocks share one ordered
/// timeline. Returns the slot ordinal, or `None` when the per-message block cap
/// is hit.
pub(super) fn push_block_slot(
    state: &mut MessageStreamState,
    kind: ContentBlockKind,
    index: Option<usize>,
) -> Option<usize> {
    if state.block_slots.len() >= MAX_BLOCKS_PER_MESSAGE {
        return None;
    }
    if let Some(index) = index {
        if let Some(ordinal) = state
            .block_slots
            .iter()
            .position(|slot| slot.index == Some(index))
        {
            // Duplicate start for the same real index reuses the existing slot.
            state.block_slots[ordinal].kind = kind;
            clear_open_indexless(state, kind);
            return Some(ordinal);
        }
    }
    let ordinal = state.block_slots.len();
    state.block_slots.push(ContentBlockSlot {
        kind,
        index,
        emitted: false,
    });
    if index.is_none() {
        state.open_indexless.open(kind, ordinal);
    } else {
        state.open_indexless.close(kind);
    }
    Some(ordinal)
}

/// Resolve the ordered slot for a partial delta or non-empty start body.
///
/// Indexed events match the slot with that real index, or allocate a late slot
/// when Claude skipped `content_block_start`. Index-less events reuse the open
/// same-kind slot, or allocate one when start was omitted.
pub(super) fn resolve_partial_slot(
    state: &mut MessageStreamState,
    explicit_index: Option<usize>,
    kind: ContentBlockKind,
) -> Option<usize> {
    if let Some(index) = explicit_index {
        if let Some(ordinal) = state
            .block_slots
            .iter()
            .position(|slot| slot.index == Some(index))
        {
            clear_open_indexless(state, kind);
            return Some(ordinal);
        }
        return push_block_slot(state, kind, Some(index));
    }

    if let Some(ordinal) = state.open_indexless.get(kind) {
        return Some(ordinal);
    }
    push_block_slot(state, kind, None)
}

/// Mark a resolved slot as presented.
///
/// Returns `false` only when the ordinal is out of range. Caps are enforced at
/// slot allocation (`push_block_slot` / `resolve_partial_slot`); once a slot
/// exists, marking it is always allowed so partial and complete paths share one
/// `emitted` flag.
pub(super) fn mark_slot_emitted(
    state: &mut MessageStreamState,
    ordinal: usize,
    explicit_index: Option<usize>,
) -> bool {
    let Some(slot) = state.block_slots.get_mut(ordinal) else {
        return false;
    };
    // Keep any known real index on the slot so later complete envelopes can
    // reconcile by stream index even when the claim path supplied it late.
    if slot.index.is_none() {
        if let Some(index) = explicit_index {
            slot.index = Some(index);
        }
    }
    slot.emitted = true;
    true
}

/// Ensure a complete-envelope content index has a slot and mark it emitted.
///
/// Used when the complete path presents (or acknowledges) a block without going
/// through partial `content_block_start`. Returns `false` only when the
/// per-message slot cap is hit and no existing slot matches the index.
pub(super) fn mark_complete_index(state: &mut MessageStreamState, index: usize) -> bool {
    if let Some(ordinal) = state
        .block_slots
        .iter()
        .position(|slot| slot.index == Some(index))
    {
        return mark_slot_emitted(state, ordinal, Some(index));
    }
    // Prefer aligning with an existing index-less slot at this content position
    // so progressive partials and complete envelopes share one timeline.
    if let Some(slot) = state.block_slots.get_mut(index) {
        if slot.index.is_none() {
            slot.index = Some(index);
            slot.emitted = true;
            return true;
        }
    }
    let Some(ordinal) = push_block_slot(state, ContentBlockKind::Other, Some(index)) else {
        return false;
    };
    mark_slot_emitted(state, ordinal, Some(index))
}

fn clear_open_indexless(state: &mut MessageStreamState, kind: ContentBlockKind) {
    state.open_indexless.close(kind);
}

pub(super) fn clear_all_open_indexless(state: &mut MessageStreamState) {
    state.open_indexless.close_all();
}

/// Decide whether a complete-envelope block was already presented by a slot.
///
/// Prefer a slot that carries this real stream index. Otherwise use the ordered
/// slot at the same content position when it has no real index (index-less
/// partials). Empty starts still occupy a slot, so complete-only bodies align
/// ahead of later streamed same-kind blocks.
pub(super) fn reconcile_complete_block(
    state: &mut MessageStreamState,
    kind: ContentBlockKind,
    complete_index: usize,
) -> bool {
    if let Some(slot) = state
        .block_slots
        .iter()
        .find(|slot| slot.index == Some(complete_index))
    {
        // Slot opened but never presented; complete envelope owns emission.
        return slot.emitted;
    }
    let Some(slot) = state.block_slots.get(complete_index) else {
        return false;
    };
    // Index-less slots align by stream order with complete content positions.
    if slot.index.is_some() {
        return false;
    }
    if slot.kind != kind && slot.kind != ContentBlockKind::Other {
        return false;
    }
    slot.emitted
}

/// Mark a complete-envelope block presented and emit text effects.
pub(super) fn mark_and_text(
    state: &mut MessageStreamState,
    index: usize,
    text: &str,
) -> Option<Vec<StreamEffect>> {
    if text.is_empty() {
        return Some(Vec::new());
    }
    if !mark_complete_index(state, index) {
        return None;
    }
    Some(text_effects(text))
}

/// Mark a complete-envelope block presented and emit reasoning effects.
pub(super) fn mark_and_reasoning(
    state: &mut MessageStreamState,
    index: usize,
    text: &str,
) -> Option<Vec<StreamEffect>> {
    if text.is_empty() {
        return Some(Vec::new());
    }
    if !mark_complete_index(state, index) {
        return None;
    }
    Some(reasoning_effects(text))
}

pub(super) fn fidelity_notice(message: &str) -> Vec<StreamEffect> {
    vec![StreamEffect::Attachment(AttachmentEvent::Notice(
        message.into(),
    ))]
}

pub(super) fn map_system(message: SystemMessage) -> Vec<StreamEffect> {
    let mut effects = Vec::new();
    // High-frequency system subtypes are progress noise under the system
    // envelope. Keep session id if present, but do not emit a notice per pulse.
    // `thinking_tokens` in particular can fire dozens of times per turn.
    let is_quiet_subtype = matches!(
        message.subtype.as_deref(),
        Some("status" | "thinking_tokens")
    );
    let is_init = message.subtype.as_deref() == Some("init");

    if let Some(session_id) = message.session_id {
        let last_activity = if is_quiet_subtype {
            None
        } else if is_init {
            Some("claude init".into())
        } else {
            Some("claude system".into())
        };
        effects.push(StreamEffect::Status(StatusPatch {
            claude_session_id: Some(session_id),
            state: Some(RunState::Running),
            last_activity,
            ..StatusPatch::default()
        }));
    }
    if let Some(subtype) = message.subtype {
        if !is_quiet_subtype {
            effects.push(StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                "claude system: {subtype}"
            ))));
        }
    }
    effects
}

pub(super) fn map_rate_limit(message: RateLimitMessage) -> Vec<StreamEffect> {
    let Some(info) = message.rate_limit_info else {
        return vec![StreamEffect::Attachment(AttachmentEvent::Notice(
            "claude stream: rate_limit_event without rate_limit_info".into(),
        ))];
    };
    let label = describe_rate_limit(&info);
    vec![
        StreamEffect::Attachment(AttachmentEvent::Notice(format!("claude limits: {label}"))),
        StreamEffect::RateLimit(info),
    ]
}

pub(super) fn map_error_message(message: ErrorMessage) -> Vec<StreamEffect> {
    // Protocol `type:error` is pending metadata only. Session waits for child
    // exit, combines this with any later `result`/exit, and emits exactly one
    // terminal Failed/Completed attachment. Never terminalize RunState here.
    let detail = bound_text(
        &message
            .result
            .or(message.subtype)
            .unwrap_or_else(|| "claude reported an error".into()),
        MAX_RESULT_CHARS,
        "error",
    );
    vec![
        StreamEffect::Status(StatusPatch {
            error: Some(detail.clone()),
            last_activity: Some("error received".into()),
            ..StatusPatch::default()
        }),
        StreamEffect::Terminal(TerminalResult {
            classification: TerminalClassification::Failure {
                subtype: "error".into(),
                is_error: true,
            },
            result_text: Some(detail.clone()),
            error: Some(detail),
            session_id: message.session_id,
            num_turns: None,
            usage: None,
            context: None,
            total_cost_usd: None,
            permission_denials: Vec::new(),
            stop_reason: None,
        }),
    ]
}

pub(super) fn tool_started_effects(block: &Value) -> Vec<StreamEffect> {
    let name = block
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| block.get("tool_name").and_then(Value::as_str))
        .unwrap_or("tool");
    let id = block.get("id").and_then(Value::as_str).unwrap_or("");
    let input = block.get("input");
    let mut lines = vec![if id.is_empty() {
        format!("tool {name}")
    } else {
        format!("tool {name} ({id})")
    }];
    let rendered = stringify_content(input);
    if !rendered.is_empty() && rendered != "null" {
        lines.extend(truncate_payload_lines(&rendered, MAX_TOOL_DISPLAY_LINES));
    }
    vec![
        StreamEffect::Attachment(AttachmentEvent::ToolStarted {
            display_lines: lines,
        }),
        StreamEffect::Status(StatusPatch {
            last_activity: Some(format!("tool: {name}")),
            ..StatusPatch::default()
        }),
    ]
}

pub(super) fn tool_finished_effects(
    tool_use_id: &str,
    ok: bool,
    content_text: &str,
) -> Vec<StreamEffect> {
    let mut lines = vec![format!("tool result ({tool_use_id})")];
    if !content_text.is_empty() {
        lines.extend(truncate_payload_lines(content_text, MAX_TOOL_DISPLAY_LINES));
    }
    vec![
        StreamEffect::Attachment(AttachmentEvent::ToolFinished {
            ok,
            display_style: CLAUDE_TOOL_DISPLAY_STYLE,
            display_lines: lines,
        }),
        StreamEffect::Status(StatusPatch {
            last_activity: Some(format!("tool result: {tool_use_id}")),
            ..StatusPatch::default()
        }),
    ]
}

pub(super) fn text_effects(text: &str) -> Vec<StreamEffect> {
    if text.is_empty() {
        return Vec::new();
    }
    let text = bound_delta_text(text, "text");
    vec![
        StreamEffect::Attachment(AttachmentEvent::AssistantTextDelta(text.clone())),
        StreamEffect::Status(StatusPatch {
            last_activity: Some("assistant text".into()),
            append_text: Some(text),
            ..StatusPatch::default()
        }),
    ]
}

pub(super) fn reasoning_effects(text: &str) -> Vec<StreamEffect> {
    if text.is_empty() {
        return Vec::new();
    }
    let text = bound_delta_text(text, "reasoning");
    vec![
        StreamEffect::Attachment(AttachmentEvent::ReasoningDelta(text)),
        StreamEffect::Status(StatusPatch {
            last_activity: Some("reasoning".into()),
            ..StatusPatch::default()
        }),
    ]
}

/// Apply a status patch onto a live RunStatus.
pub(crate) fn apply_status_patch(status: &mut RunStatus, patch: StatusPatch) {
    if let Some(state) = patch.state {
        // Never let stream patches demote a terminal state back to nonterminal.
        // Canonical disk writes also enforce this via `subagent::write_status`.
        if !status.state.is_terminal() || state.is_terminal() {
            status.state = state;
        }
    }
    if let Some(turns) = patch.turns {
        status.turns = turns;
    }
    if let Some(input_tokens) = patch.input_tokens {
        status.input_tokens = Some(input_tokens);
    }
    if let Some(output_tokens) = patch.output_tokens {
        status.output_tokens = Some(output_tokens);
    }
    if let Some(activity) = patch.last_activity {
        status.last_activity = Some(activity);
    }
    if let Some(text) = patch.append_text {
        append_tail(
            status.last_text.get_or_insert_with(String::new),
            &text,
            LAST_TEXT_BYTES,
        );
    }
    if let Some(result) = patch.result {
        status.result = Some(result);
    }
    if let Some(error) = patch.error {
        status.error = Some(error);
    }
    if let Some(session_id) = patch.claude_session_id {
        status.claude_session_id = Some(session_id);
    }
    if let Some(cost) = patch.total_cost_usd {
        status.total_cost_usd = Some(cost);
    }
}
