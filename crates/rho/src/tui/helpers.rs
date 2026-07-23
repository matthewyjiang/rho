//! Shared free helpers for the interactive TUI.

use std::io::Write;

use ratatui::text::{Line, Span};
use tokio::sync::oneshot;

use rho_providers::model::{
    catalog::LoginTarget, image_summary, ContentBlock, ImageContent, ModelMetadata, ModelUsage,
};

use super::{
    entry_lines, estimated_cost_usd_micros, styled_line, truncate_one_line, Entry,
    FinalAnswerDelta, LineFill, PasteSegment, QuestionnaireReply, SecretInput, Theme,
    PASTE_COLLAPSE_MIN_CHARS, PASTE_COLLAPSE_MIN_LINES,
};

pub(super) async fn questionnaire_reply(
    pending: &mut Option<(
        rho_sdk::ToolCallId,
        rho_sdk::HostInputId,
        oneshot::Receiver<QuestionnaireReply>,
    )>,
) -> Option<(
    rho_sdk::ToolCallId,
    rho_sdk::HostInputId,
    QuestionnaireReply,
)> {
    let (call_id, request_id, receiver) = pending.as_mut()?;
    let call_id = call_id.clone();
    let request_id = request_id.clone();
    let reply = receiver.await.ok();
    pending.take();
    reply.map(|reply| (call_id, request_id, reply))
}

pub(super) fn is_tool_entry(entry: &Entry) -> bool {
    matches!(entry, Entry::Tool(_))
}

pub(super) fn expandable_tool_entry(entry: &Entry, max_tool_output_lines: usize) -> bool {
    matches!(entry, Entry::Tool(tool) if tool_display_line_count(&tool.display_lines) > max_tool_output_lines)
}

pub(super) fn final_answer_delta<'a>(emitted_text: &str, answer: &'a str) -> FinalAnswerDelta<'a> {
    match answer.strip_prefix(emitted_text) {
        Some("") => FinalAnswerDelta::None,
        Some(suffix) => FinalAnswerDelta::Append(suffix),
        None => FinalAnswerDelta::Mismatch,
    }
}

pub(super) fn visible_composer_start(
    cursor_line: usize,
    line_count: usize,
    visible_count: usize,
) -> usize {
    if visible_count == 0 || visible_count >= line_count {
        return 0;
    }
    cursor_line
        .saturating_add(1)
        .saturating_sub(visible_count)
        .min(line_count.saturating_sub(visible_count))
}

pub(super) fn recovered_history_tail(
    entries: &[Entry],
    width: usize,
    line_limit: usize,
    max_tool_output_lines: usize,
) -> (usize, Vec<Entry>) {
    let mut selected_start = entries.len();
    let mut line_count = 0usize;
    let mut next_is_tool = false;

    for (index, entry) in entries.iter().enumerate().rev() {
        let spacing = is_tool_entry(entry) && next_is_tool;
        let entry_line_count =
            entry_lines(entry, width, max_tool_output_lines).len() + usize::from(spacing);
        if selected_start < entries.len() && line_count + entry_line_count > line_limit {
            break;
        }
        selected_start = index;
        line_count += entry_line_count;
        next_is_tool = is_tool_entry(entry);
    }

    (selected_start, entries[selected_start..].to_vec())
}

pub(super) fn tool_display_line_count(display_lines: &[String]) -> usize {
    display_lines
        .iter()
        .map(|line| line.lines().count().max(1))
        .sum()
}

pub(super) fn text_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn render_message_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.clone()),
            ContentBlock::Image(image) => Some(format!("[image: {}]", image_summary(image))),
            ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn secret_input_lines(secret: &SecretInput, width: usize) -> Vec<Line<'static>> {
    let masked = "•".repeat(secret.value.chars().count());
    vec![
        styled_line(
            truncate_one_line(
                &format!("enter {}  enter save, esc cancel", secret.target.label),
                width,
            ),
            width,
            Theme::dim(),
            LineFill::Natural,
        ),
        styled_line(
            truncate_one_line(&masked, width),
            width,
            Theme::text(),
            LineFill::Natural,
        ),
    ]
}

pub(super) fn usage_with_estimated_cost(
    mut usage: ModelUsage,
    metadata: Option<&ModelMetadata>,
) -> ModelUsage {
    if usage.cost_usd_micros.is_none() {
        usage.cost_usd_micros = estimated_cost_usd_micros(&usage, metadata);
    }
    usage
}

pub(super) fn usage_difference(usage: &ModelUsage, baseline: Option<&ModelUsage>) -> ModelUsage {
    let baseline = baseline.cloned().unwrap_or_default();
    ModelUsage {
        input_tokens: subtract_optional(usage.input_tokens, baseline.input_tokens),
        output_tokens: subtract_optional(usage.output_tokens, baseline.output_tokens),
        cache_read_tokens: subtract_optional(usage.cache_read_tokens, baseline.cache_read_tokens),
        cache_write_tokens: subtract_optional(
            usage.cache_write_tokens,
            baseline.cache_write_tokens,
        ),
        total_tokens: subtract_optional(usage.total_tokens, baseline.total_tokens),
        context_window: usage.context_window,
        cost_usd_micros: subtract_optional(usage.cost_usd_micros, baseline.cost_usd_micros),
    }
}

pub(super) fn subtract_optional(value: Option<u64>, baseline: Option<u64>) -> Option<u64> {
    value.map(|value| value.saturating_sub(baseline.unwrap_or_default()))
}

pub(super) fn merge_usage(total: &mut Option<ModelUsage>, mut usage: ModelUsage) {
    usage.total_tokens = usage.total_tokens.or_else(|| usage_total_tokens(&usage));
    let Some(total) = total.as_mut() else {
        *total = Some(usage);
        return;
    };
    total.input_tokens = add_optional(total.input_tokens, usage.input_tokens);
    total.output_tokens = add_optional(total.output_tokens, usage.output_tokens);
    total.cache_read_tokens = add_optional(total.cache_read_tokens, usage.cache_read_tokens);
    total.cache_write_tokens = add_optional(total.cache_write_tokens, usage.cache_write_tokens);
    total.total_tokens = add_optional(total.total_tokens, usage.total_tokens);
    total.cost_usd_micros = add_optional(total.cost_usd_micros, usage.cost_usd_micros);
    total.context_window = usage.context_window.or(total.context_window);
}

pub(super) fn usage_total_tokens(usage: &ModelUsage) -> Option<u64> {
    let total = usage
        .total_input_tokens()
        .unwrap_or_default()
        .saturating_add(usage.output_tokens.unwrap_or_default());
    (total > 0).then_some(total)
}

pub(super) fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

pub(super) fn oauth_pending_lines(target: &LoginTarget, width: usize) -> Vec<Line<'static>> {
    vec![styled_line(
        truncate_one_line(
            &format!("waiting for {} OAuth login  esc cancel", target.provider),
            width,
        ),
        width,
        Theme::dim(),
        LineFill::Natural,
    )]
}

pub(super) fn padded_content_width(width: usize) -> usize {
    width.saturating_sub(2).max(1)
}

pub(super) fn pad_display_line(line: Line<'static>) -> Line<'static> {
    let edge_style = line
        .spans
        .first()
        .map(|span| span.style)
        .unwrap_or_default();
    let mut spans = Vec::with_capacity(line.spans.len() + 2);
    spans.push(Span::styled(" ", edge_style));
    spans.extend(line.spans);
    spans.push(Span::styled(" ", edge_style));
    Line::from(spans)
}

pub(super) fn print_exit_summary(summary: Option<&str>) -> std::io::Result<()> {
    let Some(summary) = summary else {
        return Ok(());
    };
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{summary}")?;
    stdout.flush()
}

pub(super) fn previous_word_boundary(input: &str, cursor: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut index = cursor.min(chars.len());
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    while index > 0 && !chars[index - 1].is_whitespace() {
        index -= 1;
    }
    index
}

pub(super) fn next_word_boundary(input: &str, cursor: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut index = cursor.min(chars.len());
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    while index < chars.len() && !chars[index].is_whitespace() {
        index += 1;
    }
    index
}

pub(super) fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub(super) fn paste_marker_for(text: &str) -> Option<String> {
    let line_count = text.split('\n').count();
    let char_count = text.chars().count();
    if line_count >= PASTE_COLLAPSE_MIN_LINES {
        Some(format!("[ pasted: {line_count} lines ]"))
    } else if char_count > PASTE_COLLAPSE_MIN_CHARS {
        Some(format!("[ pasted: {char_count} chars ]"))
    } else {
        None
    }
}

pub(super) fn expand_paste_segments(input: &str, segments: &[PasteSegment]) -> String {
    if segments.is_empty() {
        return input.to_string();
    }

    let mut result = String::new();
    let mut cursor = 0;
    for segment in segments {
        if cursor > segment.start || segment.end() > input.chars().count() {
            continue;
        }
        result.extend(input.chars().skip(cursor).take(segment.start - cursor));
        result.push_str(&segment.content);
        cursor = segment.end();
    }
    result.extend(input.chars().skip(cursor));
    result
}

pub(super) fn render_user_entry(prompt: &str, images: &[ImageContent]) -> String {
    let mut parts = Vec::new();
    if !prompt.is_empty() {
        parts.push(prompt.to_string());
    }
    parts.extend(
        images
            .iter()
            .enumerate()
            .map(|(index, image)| format!("[image {}: {}]", index + 1, image_summary(image))),
    );
    parts.join("\n")
}

pub(super) fn short_session_id(id: &str) -> String {
    id.chars().take(8).collect()
}

pub(super) fn slash_command_args(input: &str) -> &str {
    let token_end = input
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(input.len());
    input[token_end..].trim_start()
}

pub(super) fn complete_slash_command(input: &str, cursor: usize, name: &str) -> (String, usize) {
    let token_end = input
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(input.len());
    let token_len = input[..token_end].chars().count();
    let args = slash_command_args(input);
    let completed = if args.is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {args}")
    };
    let completed_token_len = name.chars().count() + 1;
    let new_cursor = if cursor <= token_len {
        completed_token_len
    } else {
        completed
            .chars()
            .count()
            .min(completed_token_len.saturating_add(cursor.saturating_sub(token_len)))
    };
    (completed, new_cursor)
}
