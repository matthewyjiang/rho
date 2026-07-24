//! Presentation helpers shared by the Claude stream mapper.

use serde_json::Value;

use crate::{
    subagent::{RunState, RunStatus},
    tui::AttachmentEvent,
};

use super::format::{
    append_tail, bound_delta_text, bound_text, stringify_content, truncate_payload_lines,
    LAST_TEXT_BYTES, MAX_TOOL_DISPLAY_LINES,
};
use super::types::{
    StatusPatch, StreamEffect, TerminalClassification, TerminalResult, MAX_RESULT_CHARS,
};
use super::{describe_rate_limit, MessageStreamState, StreamEnvelope, CLAUDE_TOOL_DISPLAY_STYLE};

/// Bound on content-block indices recorded per message.
pub(super) const MAX_BLOCKS_PER_MESSAGE: usize = 256;

pub(super) fn stable_message_id(message: Option<&Value>) -> Option<String> {
    message
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

pub(super) fn block_index(event: &Value) -> Option<usize> {
    event
        .get("index")
        .and_then(Value::as_u64)
        .map(|index| index as usize)
}

pub(super) fn record_block(state: &mut MessageStreamState, index: usize) -> bool {
    if state.emitted_blocks.contains(&index) {
        return true;
    }
    if state.emitted_blocks.len() >= MAX_BLOCKS_PER_MESSAGE {
        return false;
    }
    state.emitted_blocks.insert(index);
    true
}

pub(super) fn mark_and_text(
    state: &mut MessageStreamState,
    index: usize,
    text: &str,
) -> Option<Vec<StreamEffect>> {
    if text.is_empty() {
        return Some(Vec::new());
    }
    if !record_block(state, index) {
        return None;
    }
    Some(text_effects(text))
}

pub(super) fn mark_and_reasoning(
    state: &mut MessageStreamState,
    index: usize,
    text: &str,
) -> Option<Vec<StreamEffect>> {
    if text.is_empty() {
        return Some(Vec::new());
    }
    if !record_block(state, index) {
        return None;
    }
    Some(reasoning_effects(text))
}

pub(super) fn fidelity_notice(message: &str) -> Vec<StreamEffect> {
    vec![StreamEffect::Attachment(AttachmentEvent::Notice(
        message.into(),
    ))]
}

pub(super) fn map_system(message: StreamEnvelope) -> Vec<StreamEffect> {
    let mut effects = Vec::new();
    if let Some(session_id) = message.session_id {
        effects.push(StreamEffect::Status(StatusPatch {
            claude_session_id: Some(session_id),
            state: Some(RunState::Running),
            last_activity: Some("claude init".into()),
            ..StatusPatch::default()
        }));
    }
    if let Some(subtype) = message.subtype {
        effects.push(StreamEffect::Attachment(AttachmentEvent::Notice(format!(
            "claude system: {subtype}"
        ))));
    }
    effects
}

pub(super) fn map_rate_limit(message: StreamEnvelope) -> Vec<StreamEffect> {
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

pub(super) fn map_error_message(message: StreamEnvelope) -> Vec<StreamEffect> {
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
            ok: false,
            result_text: Some(detail.clone()),
            error: Some(detail),
            session_id: message.session_id,
            num_turns: None,
            usage: None,
            context: None,
            total_cost_usd: None,
            permission_denials: Vec::new(),
            stop_reason: None,
            subtype: Some("error".into()),
            is_error: Some(true),
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
        status.input_tokens = input_tokens;
    }
    if let Some(output_tokens) = patch.output_tokens {
        status.output_tokens = output_tokens;
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
