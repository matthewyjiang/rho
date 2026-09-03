//! Payload bounds, status patches, and tool-card field helpers for CLI streams.

use std::path::Path;

use serde_json::Value;

use rho_sdk::{ceil_char_boundary, ELLIPSIS};
use rho_tools::{
    tool::compact_display_path,
    tool_card::{ToolBody, ToolCard, ToolFact},
};

use crate::{run_artifacts::AttachmentEvent, subagent::RunStatus};

use super::stream_effect::{
    StatusPatch, StreamEffect, MAX_RESULT_CHARS, MAX_TEXT_DELTA_CHARS, MAX_TOOL_PAYLOAD_CHARS,
};

/// Maximum body lines kept on a finished tool card after truncation.
///
/// Collapsed paint uses `max_tool_output_lines` (default 10). This cap is
/// larger so attach expand can reveal more than a couple of extra rows. The
/// 16 KiB payload bound still limits journal size.
pub(crate) const MAX_TOOL_BODY_LINES: usize = 50;

/// Bytes retained on [`crate::subagent::RunStatus::last_text`].
pub(crate) const LAST_TEXT_BYTES: usize = 400;

pub(crate) fn bound_text(text: &str, max_chars: usize, label: &str) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text.chars().take(max_chars).collect::<String>();
    out.push_str(&format!("{ELLIPSIS} [truncated {label}]"));
    out
}

pub(crate) fn truncate_payload_lines(text: &str, max_lines: usize) -> Vec<String> {
    let bounded = bound_text(text, MAX_TOOL_PAYLOAD_CHARS, "tool payload");
    let mut lines = bounded.lines().map(str::to_string).collect::<Vec<_>>();
    if lines.len() > max_lines {
        let omitted = lines.len() - max_lines;
        lines.truncate(max_lines);
        lines.push(format!("{ELLIPSIS} {omitted} more line(s)"));
    }
    lines
}

pub(crate) fn bound_result_text(text: &str) -> String {
    bound_text(text, MAX_RESULT_CHARS, "result")
}

pub(crate) fn bound_delta_text(text: &str, label: &str) -> String {
    bound_text(text, MAX_TEXT_DELTA_CHARS, label)
}

pub(crate) fn append_tail(buffer: &mut String, text: &str, max: usize) {
    buffer.push_str(text);
    if buffer.len() > max {
        let cut = buffer.len() - max;
        let boundary = ceil_char_boundary(buffer, cut);
        *buffer = buffer[boundary..].to_string();
    }
}

pub(crate) fn set_lines_body(card: &mut ToolCard, content_text: &str) {
    if content_text.trim().is_empty() {
        return;
    }
    card.body = ToolBody::Lines(truncate_payload_lines(content_text, MAX_TOOL_BODY_LINES));
}

pub(crate) fn display_path_field(
    input: Option<&Value>,
    keys: &[&str],
    cwd: Option<&Path>,
) -> Option<String> {
    let path = string_field(input, keys)?;
    Some(match cwd {
        Some(cwd) => compact_display_path(cwd, &path),
        None => path,
    })
}

pub(crate) fn string_field(input: Option<&Value>, keys: &[&str]) -> Option<String> {
    let input = input?;
    keys.iter().find_map(|key| {
        input
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

pub(crate) fn u64_field(input: Option<&Value>, keys: &[&str]) -> Option<u64> {
    let input = input?;
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_u64))
}

pub(crate) fn count_fact(
    singular: &str,
    plural: &str,
    value: u64,
    detail: Option<String>,
) -> ToolFact {
    ToolFact::Count {
        label: if value == 1 {
            singular.into()
        } else {
            plural.into()
        },
        value,
        detail,
    }
}

pub(crate) fn quoted(text: &str, max_chars: usize) -> String {
    format!("\"{}\"", truncate(text, max_chars.saturating_sub(2)))
}

pub(crate) fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

pub(crate) fn text_effects(text: &str) -> Vec<StreamEffect> {
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

pub(crate) fn reasoning_effects(text: &str) -> Vec<StreamEffect> {
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
///
/// NEXT_MAJOR(result.json): rename claude_session_id/claude_model to runtime_session_id/runtime_model; readers branch on runtime.
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
    if let Some(model) = patch.claude_model {
        status.claude_model = Some(model);
    }
    if let Some(cost) = patch.total_cost_usd {
        status.total_cost_usd = Some(cost);
    }
}
