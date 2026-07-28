mod entry_render;

pub(super) use entry_render::{
    apply_markdown_images, entry_lines, render_entry_with_options, TrailingBlank,
};

use super::{
    feed_image::{reserve_entry_image_rows, reserve_markdown_image_rows},
    info_command::runtime_info_lines,
    limits_command::usage_limit_lines,
    message_render::{render_assistant_content, render_reasoning_content},
    rendered_entry::RenderedEntry,
    theme::Theme,
    Entry, FeedImage, PickerBadgeTone, PickerItem, UiPicker, DEFAULT_TUI_HEIGHT,
};
use rho_providers::model::{image_summary, ImageContent};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use std::borrow::Cow;

use ratatui::{
    layout::Position,
    style::{Modifier, Style},
    text::{Line, Span},
};

const MAX_PICKER_ITEMS: usize = DEFAULT_TUI_HEIGHT as usize - 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LineFill {
    Natural,
    PadToWidth,
}

impl LineFill {
    fn pads_to_width(self) -> bool {
        matches!(self, Self::PadToWidth)
    }
}

const SESSION_HEADER_HINTS: &[&str] = &[
    " shift+tab    Cycle reasoning level",
    " ctrl+c       Clear the composer",
    " /            Show available commands",
    " !            Run a shell command",
];

pub(super) fn session_header_lines(
    update_notice: Option<&str>,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw(" "),
            Span::styled("rho", Theme::brand()),
            Span::raw("  v"),
            Span::styled(env!("CARGO_PKG_VERSION"), Theme::success()),
        ]),
    ];
    if let Some(notice) = update_notice {
        // Match the brand line's leading space so the notice lines up under "rho".
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                truncate_one_line(notice, width.saturating_sub(1)),
                Theme::warning(),
            ),
        ]));
    }
    lines.push(Line::raw(""));
    push_session_header_hints(&mut lines, width);
    lines.push(Line::raw(""));
    lines
}

fn push_session_header_hints(lines: &mut Vec<Line<'static>>, width: usize) {
    for hint in SESSION_HEADER_HINTS {
        lines.push(Line::from(Span::styled(
            truncate_one_line(hint, width),
            Theme::dim(),
        )));
    }
}

pub(super) fn picker_lines(picker: &UiPicker, width: usize) -> Vec<Line<'static>> {
    list_picker_lines(picker, width)
}

fn list_picker_lines(picker: &UiPicker, width: usize) -> Vec<Line<'static>> {
    let matching_indices = picker.matching_indices();
    let mut lines = Vec::with_capacity(MAX_PICKER_ITEMS + 7);
    lines.push(picker_filter_line(picker, width));
    lines.push(Line::raw(""));

    if matching_indices.is_empty() {
        lines.push(styled_line(
            truncate_one_line("  no matches", width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ));
        lines.push(Line::raw(""));
        lines.push(styled_line(
            truncate_one_line(&picker_footer_text(picker), width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ));
        return lines;
    }

    let label_width = picker_label_width(picker, width);
    let start = visible_picker_match_start(picker, &matching_indices);
    for index in matching_indices
        .iter()
        .copied()
        .skip(start)
        .take(MAX_PICKER_ITEMS)
    {
        let item = &picker.items[index];
        let selected = index == picker.selected;
        lines.push(picker_item_line(item, selected, label_width, width));
    }

    let selected_position = matching_indices
        .iter()
        .position(|index| *index == picker.selected)
        .unwrap_or(0);
    lines.push(styled_line(
        truncate_one_line(
            &format!("  ({}/{})", selected_position + 1, matching_indices.len()),
            width,
        ),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));
    lines.push(Line::raw(""));
    if picker.has_item_details() {
        let detail = picker
            .selected_item()
            .and_then(|item| item.detail.as_deref())
            .unwrap_or_default();
        let detail = truncate_one_line(detail, width.saturating_sub(2));
        let detail = if width > 2 {
            format!("  {detail}")
        } else {
            truncate_one_line(&detail, width)
        };
        lines.push(styled_line(detail, width, Theme::dim(), LineFill::Natural));
        lines.push(Line::raw(""));
    }
    lines.push(styled_line(
        truncate_one_line(&picker_footer_text(picker), width),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));
    lines
}

fn picker_filter_line(picker: &UiPicker, width: usize) -> Line<'static> {
    if width <= 1 {
        return Line::from(Span::styled(">", Theme::text_strong()));
    }

    Line::from(vec![
        Span::styled(">", Theme::text_strong()),
        Span::raw(" "),
        Span::styled(
            truncate_one_line(&picker.filter, width.saturating_sub(2)),
            Theme::text_strong(),
        ),
    ])
}

fn picker_label_width(picker: &UiPicker, width: usize) -> usize {
    let max_label_width = match picker.action {
        super::PickerAction::SelectModel | super::PickerAction::SelectInternalAgentModel => 60,
        super::PickerAction::ResumeSession | super::PickerAction::SelectTreeNode => 60,
        super::PickerAction::Config
        | super::PickerAction::Dismiss
        | super::PickerAction::LoginGroup
        | super::PickerAction::LoginProvider
        | super::PickerAction::LogoutProvider
        | super::PickerAction::SwitchAuthMode
        | super::PickerAction::RefreshModelList
        | super::PickerAction::InsertSkillCommand
        | super::PickerAction::ViewAgent
        | super::PickerAction::EditAgent => 30,
    };
    let reserved_preview_width = width.saturating_sub(18);
    let available_width = if reserved_preview_width >= 12 {
        reserved_preview_width
    } else {
        width.saturating_sub(2).max(1)
    };
    let max_label_width = max_label_width.min(available_width);
    let min_label_width = 12.min(max_label_width).max(1);
    picker
        .items
        .iter()
        .map(|item| display_width(&item.label))
        .max()
        .unwrap_or(min_label_width)
        .clamp(min_label_width, max_label_width)
}

fn picker_item_line(
    item: &PickerItem,
    selected: bool,
    label_width: usize,
    width: usize,
) -> Line<'static> {
    let marker = if selected { "→" } else { " " };
    let row_style = if selected {
        Theme::accent()
    } else {
        Theme::text()
    };
    if width <= 1 {
        return Line::from(Span::styled(marker.to_string(), row_style));
    }

    let label_width = label_width.min(width.saturating_sub(2));
    let label = truncate_one_line(&item.label, label_width);
    let mut used_width = 2 + label_width;
    let mut spans = vec![Span::styled(
        format!(
            "{marker} {label}{}",
            " ".repeat(label_width.saturating_sub(display_width(&label)))
        ),
        row_style,
    )];
    if let Some(badge) = &item.badge {
        let remaining = width.saturating_sub(used_width.saturating_add(2));
        if remaining > 1 {
            // Value badges (config) should use free width instead of a magic cap.
            // Preview text, when present, takes whatever remains after the badge.
            let badge_text = truncate_one_line(&badge.text, remaining);
            used_width += 2 + display_width(&badge_text);
            spans.push(Span::raw("  "));
            spans.push(Span::styled(badge_text, picker_badge_style(badge.tone)));
        }
    }
    if let Some(preview) = &item.preview {
        let remaining = width.saturating_sub(used_width.saturating_add(2));
        if remaining > 1 {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                truncate_one_line(preview, remaining),
                Theme::dim(),
            ));
        }
    }
    Line::from(spans)
}

fn picker_footer_text(picker: &UiPicker) -> String {
    let action = picker.confirm_action_label();
    let pin = if picker.help.contains("ctrl-p") {
        " · Ctrl-P to pin/unpin"
    } else {
        ""
    };
    let tab = if picker.help.contains("tab") {
        " · Tab to complete"
    } else {
        ""
    };
    let escape = if picker.has_parent() {
        "back"
    } else {
        "cancel"
    };
    format!(
        "  {} · Type to search · Enter to {action}{pin}{tab} · Esc to {escape}",
        picker.title
    )
}

pub(super) fn picker_badge_style(tone: PickerBadgeTone) -> Style {
    match tone {
        PickerBadgeTone::Internal | PickerBadgeTone::Editable => Theme::accent(),
        PickerBadgeTone::Selected => Theme::warning(),
        PickerBadgeTone::Favorite | PickerBadgeTone::Healthy => Theme::success(),
        PickerBadgeTone::Warning => Theme::warning(),
    }
}

pub(super) fn visible_picker_match_start(picker: &UiPicker, matching_indices: &[usize]) -> usize {
    let selected_position = matching_indices
        .iter()
        .position(|index| *index == picker.selected)
        .unwrap_or(0);
    selected_position
        .saturating_add(1)
        .saturating_sub(MAX_PICKER_ITEMS)
}

pub(super) fn truncate_one_line(text: &str, width: usize) -> String {
    // Fast path: no newlines means we skip the replace allocation entirely.
    if !text.contains('\n') {
        if UnicodeWidthStr::width(text) <= width {
            return text.to_string();
        }
        if width <= 1 {
            return "…".chars().take(width).collect();
        }
        return format!("{}…", truncate_to_display_width(text, width - 1));
    }

    let normalized = text.replace('\n', " ");
    if UnicodeWidthStr::width(normalized.as_str()) <= width {
        return normalized;
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    format!("{}…", truncate_to_display_width(&normalized, width - 1))
}

/// Truncate from the front, keeping the end of `text` with a leading ellipsis.
pub(super) fn truncate_keep_end(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    // Fast path: no newlines means we skip the replace allocation entirely.
    if !text.contains('\n') {
        if display_width(text) <= width {
            return text.to_string();
        }
        return truncate_keep_end_owned(text, width);
    }
    let normalized = text.replace('\n', " ");
    if display_width(&normalized) <= width {
        return normalized;
    }
    truncate_keep_end_owned(&normalized, width)
}

fn truncate_keep_end_owned(text: &str, width: usize) -> String {
    if width <= 1 {
        return "…".chars().take(width).collect();
    }

    let target = width - 1;
    let mut start = text.len();
    let mut used = 0usize;
    for (index, ch) in text.char_indices().rev() {
        let ch_width = char_display_width(ch);
        if used + ch_width > target {
            break;
        }
        used += ch_width;
        start = index;
    }
    format!("…{}", &text[start..])
}

pub(super) fn display_width(text: &str) -> usize {
    text.split(char::is_control)
        .map(UnicodeWidthStr::width)
        .sum()
}

pub(super) fn char_display_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

fn truncate_to_display_width(text: &str, max_width: usize) -> Cow<'_, str> {
    if display_width(text) <= max_width {
        return Cow::Borrowed(text);
    }
    let mut end = 0;
    let mut width = 0;
    for (index, ch) in text.char_indices() {
        let ch_width = char_display_width(ch);
        if width + ch_width > max_width {
            break;
        }
        width += ch_width;
        end = index + ch.len_utf8();
    }
    Cow::Owned(text[..end].to_string())
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CompleteVisualPrefix {
    pub(super) byte_index: usize,
    pub(super) ends_with_wrap: bool,
}

#[cfg(test)]
pub(super) fn complete_visual_prefix(text: &str, width: usize) -> CompleteVisualPrefix {
    complete_visual_line_ends(text, width)
        .last()
        .copied()
        .map(|(index, ch)| CompleteVisualPrefix {
            byte_index: index,
            ends_with_wrap: ch != '\n',
        })
        .unwrap_or_default()
}

#[cfg(test)]
fn complete_visual_prefix_byte_index(text: &str, width: usize) -> usize {
    complete_visual_prefix(text, width).byte_index
}

#[cfg(test)]
fn complete_visual_line_ends(text: &str, width: usize) -> Vec<(usize, char)> {
    let width = width.max(1);
    let mut ends = Vec::new();
    let mut line_start = 0;

    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            ends.extend(complete_word_wrapped_line_ends(
                &text[line_start..index],
                line_start,
                width,
            ));
            ends.push((index + ch.len_utf8(), ch));
            line_start = index + ch.len_utf8();
        }
    }

    if line_start < text.len() {
        ends.extend(complete_word_wrapped_line_ends(
            &text[line_start..],
            line_start,
            width,
        ));
    }

    ends
}

#[cfg(test)]
fn complete_word_wrapped_line_ends(line: &str, offset: usize, width: usize) -> Vec<(usize, char)> {
    wrap_line_at_whitespace_ranges(line, width)
        .into_iter()
        .filter(|range| {
            range.end < line.len() || display_width(&line[range.clone()]) >= width.max(1)
        })
        .map(|range| (offset + range.end, 'x'))
        .collect()
}

pub(super) fn input_cursor_position(input: &str, cursor: usize, width: usize) -> Position {
    let prefix: String = input.chars().take(cursor).collect();
    let lines = editable_input_visual_lines(&prefix, width);
    Position {
        x: lines
            .last()
            .map(|line| display_width(line))
            .unwrap_or_default() as u16,
        y: lines.len().saturating_sub(1) as u16,
    }
}

pub(super) fn char_prefix_display_width(value: &str, cursor: usize) -> usize {
    value
        .chars()
        .take(cursor)
        .map(char_display_width)
        .sum::<usize>()
}

pub(super) fn input_cursor_index_on_visual_line(
    input: &str,
    visual_lines: &[String],
    target_row: usize,
    target_column: usize,
) -> usize {
    let mut line_start = 0;
    for line in visual_lines.iter().take(target_row) {
        line_start += line.chars().count();
        if input.chars().nth(line_start) == Some('\n') {
            line_start += 1;
        }
    }

    let mut cursor = line_start;
    let mut column = 0;
    if let Some(line) = visual_lines.get(target_row) {
        for ch in line.chars() {
            let next_column = column + char_display_width(ch);
            if next_column > target_column {
                break;
            }
            column = next_column;
            cursor += 1;
        }
    }
    cursor
}

pub(super) fn input_image_lines(images: &[ImageContent], width: usize) -> Vec<Line<'static>> {
    images
        .iter()
        .enumerate()
        .map(|(index, image)| {
            styled_line(
                format!("[image {}: {}]", index + 1, image_summary(image)),
                width.max(1),
                Theme::dim(),
                LineFill::Natural,
            )
        })
        .collect()
}

pub(super) fn input_lines(
    input: &str,
    width: usize,
    highlighted_range: Option<std::ops::Range<usize>>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let input_lines = editable_input_visual_lines(input, width);
    let input_chars = input.chars().collect::<Vec<_>>();
    let mut input_cursor = 0;
    for (line_index, visual_line) in input_lines.into_iter().enumerate() {
        if line_index > 0 && input_chars.get(input_cursor) == Some(&'\n') {
            input_cursor += 1;
        }
        let mut spans = Vec::new();
        let mut span_text = String::new();
        let mut span_highlighted = false;
        for character in visual_line.chars() {
            let highlighted = highlighted_range
                .as_ref()
                .is_some_and(|range| range.contains(&input_cursor));
            input_cursor += 1;
            if !span_text.is_empty() && highlighted != span_highlighted {
                let style = if span_highlighted {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                spans.push(Span::styled(std::mem::take(&mut span_text), style));
            }
            span_highlighted = highlighted;
            span_text.push(character);
        }
        if !span_text.is_empty() {
            let style = if span_highlighted {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            spans.push(Span::styled(span_text, style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

pub(super) fn editable_input_visual_lines(input: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = input_visual_lines(input, width);
    if !input.is_empty()
        && lines
            .last()
            .is_some_and(|line| display_width(line) >= width)
    {
        lines.push(String::new());
    }
    lines
}

pub(super) fn input_visual_lines(input: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for raw_line in input.split('\n') {
        let wrapped = wrap_line_hard(raw_line, width);
        if wrapped.is_empty() {
            lines.push(String::new());
        } else {
            lines.extend(wrapped);
        }
    }
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

pub(super) fn tool_entry_lines(
    tool: &super::ToolEntry,
    width: usize,
    max_tool_output_lines: usize,
) -> Vec<Line<'static>> {
    super::tool_card_render::tool_entry_lines(tool, width, max_tool_output_lines)
}

fn render_non_assistant_entry(
    lines: &mut Vec<Line<'static>>,
    entry: &Entry,
    width: usize,
    max_tool_output_lines: usize,
) {
    match entry {
        Entry::User(text) => push_wrapped_text(
            lines,
            text,
            width,
            Theme::user_message(),
            LineFill::PadToWidth,
        ),
        Entry::Assistant(_) | Entry::Reasoning(_) => {
            unreachable!("assistant and reasoning entries are rendered as markdown")
        }
        Entry::Tool(tool) => {
            super::tool_card_render::push_tool_card(
                lines,
                &tool.card,
                width,
                max_tool_output_lines,
                tool.expanded,
            );
        }
        Entry::Notice(text) => {
            push_wrapped_text(lines, text, width, Theme::dim_italic(), LineFill::Natural)
        }
        Entry::RuntimeInfo(info) => lines.extend(runtime_info_lines(info, width)),
        Entry::UsageLimits(limits) => lines.extend(usage_limit_lines(limits, width)),
        Entry::Error(text) => {
            push_wrapped_text(lines, text, width, Theme::error(), LineFill::Natural)
        }
    }
}

pub(super) fn push_wrapped_text(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    style: Style,
    fill: LineFill,
) {
    push_wrapped_text_with(lines, text, width, style, fill, wrap_line_at_whitespace);
}

pub(super) fn push_wrapped_text_with(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    style: Style,
    fill: LineFill,
    wrap_line: fn(&str, usize) -> Vec<String>,
) {
    let width = width.max(1);
    let mut emitted = false;
    for raw_line in text.lines() {
        let chunks = wrap_line(raw_line, width);
        for chunk in chunks {
            lines.push(styled_line(chunk, width, style, fill));
            emitted = true;
        }
    }

    if !emitted {
        lines.push(styled_line(String::new(), width, style, fill));
    }
}

pub(super) fn styled_line(
    mut text: String,
    width: usize,
    style: Style,
    fill: LineFill,
) -> Line<'static> {
    if fill.pads_to_width() {
        let len = display_width(&text);
        if len < width {
            text.push_str(&" ".repeat(width - len));
        }
    }
    Line::from(Span::styled(text, style))
}

pub(super) fn padded_inner_width(width: usize) -> usize {
    width.saturating_sub(2).max(1)
}

pub(super) fn pad_entry_line(line: Line<'static>) -> Line<'static> {
    pad_line(line)
}

fn pad_line(line: Line<'static>) -> Line<'static> {
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

pub(super) fn styled_blank_line(width: usize, style: Style) -> Line<'static> {
    Line::from(Span::styled(" ".repeat(width.max(1)), style))
}

pub(super) fn wrap_line_at_whitespace(line: &str, width: usize) -> Vec<String> {
    wrap_line_at_whitespace_ranges(line, width)
        .into_iter()
        .map(|range| line[range].to_string())
        .collect()
}

pub(super) fn wrap_line_at_whitespace_ranges(
    line: &str,
    width: usize,
) -> Vec<std::ops::Range<usize>> {
    wrap_line_at_whitespace_ranges_with_protected_prefix(line, width, 0)
}

/// Wrap at whitespace without allowing the first break to strand a semantic prefix.
///
/// `protected_prefix_end` is a byte offset whose preceding whitespace cannot be
/// used as the first wrap point. If the following token overflows, the first
/// line is filled to `width` instead.
pub(super) fn wrap_line_at_whitespace_ranges_with_protected_prefix(
    line: &str,
    width: usize,
    protected_prefix_end: usize,
) -> Vec<std::ops::Range<usize>> {
    let width = width.max(1);
    if line.is_empty() {
        return std::iter::once(0..0).collect();
    }

    let mut ranges = Vec::new();
    let mut start = 0;
    while start < line.len() {
        let mut count = 0usize;
        let mut last_fitting_split = None;
        let mut whitespace_break = None;
        let mut saw_non_whitespace = false;
        let mut overflow = false;
        let mut prefer_width_split = false;

        for (relative_index, ch) in line[start..].char_indices() {
            let ch_width = char_display_width(ch);
            if count > 0 && count + ch_width > width {
                overflow = true;
                prefer_width_split = ch.is_whitespace();
                break;
            }

            count += ch_width;
            let next = start + relative_index + ch.len_utf8();
            last_fitting_split = Some(next);
            if ch.is_whitespace() {
                if saw_non_whitespace {
                    whitespace_break = Some(next);
                }
            } else {
                saw_non_whitespace = true;
            }
        }

        if !overflow {
            ranges.push(start..line.len());
            break;
        }

        let split = if prefer_width_split
            || (start == 0 && whitespace_break.is_some_and(|split| split <= protected_prefix_end))
        {
            last_fitting_split.expect("overflow requires a fitting split")
        } else {
            whitespace_break
                .filter(|split| *split > start)
                .unwrap_or_else(|| last_fitting_split.expect("overflow requires a fitting split"))
        };
        ranges.push(start..split);
        start = split;
    }

    ranges
}

pub(super) fn wrap_line_hard(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for ch in line.chars() {
        let ch_width = char_display_width(ch);
        if current_width > 0 && current_width + ch_width > width {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
        if current_width >= width {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Display width of concatenated styled spans.
pub(super) fn spans_display_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

/// Slice concatenated spans by byte offsets into their joined UTF-8 text.
pub(super) fn slice_spans_by_bytes(
    spans: &[Span<'static>],
    start: usize,
    end: usize,
) -> Vec<Span<'static>> {
    if start >= end {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    for span in spans {
        let content = span.content.as_ref();
        let span_start = offset;
        let span_end = offset + content.len();
        offset = span_end;
        if span_end <= start || span_start >= end {
            continue;
        }
        let from = start.saturating_sub(span_start);
        let to = (end - span_start).min(content.len());
        if from >= to {
            continue;
        }
        // Ranges come from the concatenated UTF-8 text, so byte edges are char edges.
        out.push(Span::styled(content[from..to].to_string(), span.style));
    }
    out
}

/// Hard-wrap multi-span content to `width` display columns, preserving styles.
pub(super) fn wrap_spans_hard(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut used = 0;

    for span in spans {
        let mut chunk = String::new();
        for character in span.content.chars() {
            if character == '\n' {
                push_span_chunk(&mut row, &mut chunk, span.style);
                rows.push(std::mem::take(&mut row));
                used = 0;
                continue;
            }
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if used > 0 && used + character_width > width {
                push_span_chunk(&mut row, &mut chunk, span.style);
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            chunk.push(character);
            used += character_width;
            if used >= width {
                push_span_chunk(&mut row, &mut chunk, span.style);
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
        }
        push_span_chunk(&mut row, &mut chunk, span.style);
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

fn push_span_chunk(row: &mut Vec<Span<'static>>, chunk: &mut String, style: Style) {
    if !chunk.is_empty() {
        row.push(Span::styled(std::mem::take(chunk), style));
    }
}

pub(super) fn labeled_divider_line(
    labels: &[&str],
    style: Style,
    width: usize,
) -> Option<Line<'static>> {
    const PREFIX: &str = "─ ";
    const MIN_SUFFIX: usize = 2;
    let prefix_width = display_width(PREFIX);
    for label in labels {
        let label_width = display_width(label);
        let needed = prefix_width
            .saturating_add(label_width)
            .saturating_add(1)
            .saturating_add(MIN_SUFFIX);
        if needed > width {
            continue;
        }
        let suffix_width = width
            .saturating_sub(prefix_width)
            .saturating_sub(label_width)
            .saturating_sub(1);
        return Some(Line::from(vec![
            Span::styled(PREFIX.to_string(), style),
            Span::styled(format!("{label} "), style),
            Span::styled("─".repeat(suffix_width), style),
        ]));
    }
    None
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

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
