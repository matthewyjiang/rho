use super::{Entry, PickerBadgeTone, PickerItem, TuiInfo, UiPicker, INLINE_VIEWPORT_HEIGHT};
use crate::tool::{ToolDisplayStyle, ToolRgb};
use ratatui::{
    layout::Position,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

const MAX_PICKER_ITEMS: usize = INLINE_VIEWPORT_HEIGHT as usize - 12;

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

pub(super) fn session_header_lines(_info: &TuiInfo, width: usize) -> Vec<Line<'static>> {
    let divider = "─".repeat(width.max(1));
    vec![
        Line::styled(divider.clone(), Style::default().fg(Color::DarkGray)),
        Line::from(vec![
            Span::styled(
                "rho",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  v"),
            Span::styled(
                env!("CARGO_PKG_VERSION"),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::styled(divider, Style::default().fg(Color::DarkGray)),
        Line::raw(""),
    ]
}

pub(super) fn picker_lines(picker: &UiPicker, width: usize) -> Vec<Line<'static>> {
    let matching_indices = picker.matching_indices();
    let mut lines = Vec::with_capacity(MAX_PICKER_ITEMS + 7);
    lines.push(picker_filter_line(picker));
    lines.push(Line::raw(""));

    if matching_indices.is_empty() {
        lines.push(styled_line(
            "  no matches".to_string(),
            width,
            Style::default().fg(Color::DarkGray),
            LineFill::Natural,
        ));
        lines.push(Line::raw(""));
        lines.push(styled_line(
            picker_footer_text(picker),
            width,
            Style::default().fg(Color::DarkGray),
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
        format!("  ({}/{})", selected_position + 1, matching_indices.len()),
        width,
        Style::default().fg(Color::DarkGray),
        LineFill::Natural,
    ));
    lines.push(Line::raw(""));
    if let Some(detail) = picker
        .selected_item()
        .and_then(|item| item.detail.as_deref())
    {
        lines.push(styled_line(
            format!(
                "  {}",
                truncate_one_line(detail, width.saturating_sub(2).max(1))
            ),
            width,
            Style::default().fg(Color::DarkGray),
            LineFill::Natural,
        ));
        lines.push(Line::raw(""));
    }
    lines.push(styled_line(
        picker_footer_text(picker),
        width,
        Style::default().fg(Color::DarkGray),
        LineFill::Natural,
    ));
    lines
}

fn picker_filter_line(picker: &UiPicker) -> Line<'static> {
    Line::from(vec![
        Span::styled(">", Style::default().fg(Color::White)),
        Span::raw(" "),
        Span::styled(picker.filter.clone(), Style::default().fg(Color::White)),
    ])
}

fn picker_label_width(picker: &UiPicker, width: usize) -> usize {
    let max_label_width = match picker.action {
        super::PickerAction::SelectModel | super::PickerAction::SelectTitleModel => 60,
        super::PickerAction::ResumeSession => 36,
        super::PickerAction::Config
        | super::PickerAction::LoginProvider
        | super::PickerAction::LogoutProvider
        | super::PickerAction::InsertSkillCommand => 30,
    };
    picker
        .items
        .iter()
        .map(|item| item.label.chars().count())
        .max()
        .unwrap_or(12)
        .clamp(12, max_label_width)
        .min(width.saturating_sub(18).max(12))
}

fn picker_item_line(
    item: &PickerItem,
    selected: bool,
    label_width: usize,
    width: usize,
) -> Line<'static> {
    let marker = if selected { "→" } else { " " };
    let row_style = if selected {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::White)
    };
    let label = truncate_one_line(&item.label, label_width);
    let mut used_width = 2 + label_width;
    let mut spans = vec![Span::styled(
        format!("{marker} {label:<label_width$}"),
        row_style,
    )];
    if let Some(badge) = &item.badge {
        let badge_text = truncate_one_line(&badge.text, 24);
        used_width += 2 + badge_text.chars().count();
        spans.push(Span::raw("  "));
        spans.push(Span::styled(badge_text, picker_badge_style(badge.tone)));
    }
    if let Some(preview) = &item.preview {
        let remaining = width.saturating_sub(used_width.saturating_add(2));
        if remaining > 1 {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                truncate_one_line(preview, remaining),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    Line::from(spans)
}

fn picker_footer_text(picker: &UiPicker) -> String {
    let action = match picker.action {
        super::PickerAction::Config => "change",
        super::PickerAction::SelectModel
        | super::PickerAction::SelectTitleModel
        | super::PickerAction::LoginProvider
        | super::PickerAction::LogoutProvider
        | super::PickerAction::InsertSkillCommand
        | super::PickerAction::ResumeSession => "select",
    };
    let tab = if picker.help.contains("tab") {
        " · Tab to complete"
    } else {
        ""
    };
    format!(
        "  {} · Type to search · Enter to {action}{tab} · Esc to cancel",
        picker.title
    )
}

fn picker_badge_style(tone: PickerBadgeTone) -> Style {
    match tone {
        PickerBadgeTone::Selected => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
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
    let mut text = text.replace('\n', " ");
    if text.chars().count() <= width {
        return text;
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    text = text.chars().take(width - 1).collect();
    text.push('…');
    text
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CompleteVisualPrefix {
    pub(super) byte_index: usize,
    pub(super) ends_with_wrap: bool,
}

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

fn complete_word_wrapped_line_ends(line: &str, offset: usize, width: usize) -> Vec<(usize, char)> {
    wrap_line_at_whitespace_ranges(line, width)
        .into_iter()
        .filter(|range| {
            range.end < line.len() || line[range.clone()].chars().count() >= width.max(1)
        })
        .map(|range| (offset + range.end, 'x'))
        .collect()
}

pub(super) fn input_cursor_position(input: &str, cursor: usize, width: usize) -> Position {
    let prefix: String = input.chars().take(cursor).collect();
    let lines = input_visual_lines(&prefix, width);
    Position {
        x: lines
            .last()
            .map(|line| line.chars().count())
            .unwrap_or_default() as u16,
        y: lines.len().saturating_sub(1) as u16,
    }
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

pub(super) fn entry_lines(
    entry: &Entry,
    width: usize,
    max_tool_output_lines: usize,
) -> Vec<Line<'static>> {
    let inner_width = padded_inner_width(width);
    let mut lines = Vec::new();
    match entry {
        Entry::User(text) => push_wrapped_text(
            &mut lines,
            text,
            inner_width,
            Style::default().fg(Color::White).bg(Color::Rgb(36, 44, 54)),
            LineFill::PadToWidth,
        ),
        Entry::Assistant(text) => push_wrapped_text(
            &mut lines,
            text,
            inner_width,
            Style::default(),
            LineFill::Natural,
        ),
        Entry::Reasoning(text) => push_wrapped_text(
            &mut lines,
            text,
            inner_width,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
            LineFill::Natural,
        ),
        Entry::Tool {
            ok,
            display_style,
            display_lines,
            expanded,
            ..
        } => push_tool_block(
            &mut lines,
            *ok,
            display_lines,
            *display_style,
            inner_width,
            max_tool_output_lines,
            *expanded,
        ),
        Entry::Notice(text) => push_wrapped_text(
            &mut lines,
            text,
            inner_width,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
            LineFill::Natural,
        ),
        Entry::Error(text) => push_wrapped_text(
            &mut lines,
            text,
            inner_width,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            LineFill::Natural,
        ),
    }

    let block_style = lines
        .first()
        .and_then(|line| line.spans.first())
        .map(|span| span.style)
        .unwrap_or_default();
    let mut padded = Vec::with_capacity(lines.len() + 2);
    padded.push(styled_blank_line(width, block_style));
    padded.extend(lines.into_iter().map(pad_line));
    padded.push(styled_blank_line(width, block_style));
    padded
}

fn push_tool_block(
    lines: &mut Vec<Line<'static>>,
    ok: bool,
    display_lines: &[String],
    display_style: ToolDisplayStyle,
    width: usize,
    max_tool_output_lines: usize,
    expanded: bool,
) {
    let background = if ok {
        display_style.success_background
    } else {
        display_style.failure_background
    };
    let style = Style::default()
        .fg(tool_color(display_style.foreground))
        .bg(tool_color(background));

    let logical_lines = tool_logical_lines(display_lines);
    let max_tool_output_lines = max_tool_output_lines.max(1);
    let truncated = logical_lines.len() > max_tool_output_lines;
    let visible_count = if truncated && !expanded {
        max_tool_output_lines
    } else {
        logical_lines.len()
    };

    for line in logical_lines.iter().take(visible_count) {
        push_hard_wrapped_text(lines, line, width, style, LineFill::PadToWidth);
    }

    if truncated {
        let prompt = if expanded {
            "ctrl+o to collapse".to_string()
        } else {
            format!(
                "... {} more lines, ctrl+o to expand",
                logical_lines.len() - visible_count
            )
        };
        push_wrapped_text(lines, &prompt, width, style, LineFill::PadToWidth);
    }
}

fn tool_logical_lines(display_lines: &[String]) -> Vec<String> {
    display_lines
        .iter()
        .flat_map(|line| {
            let lines = line.lines().map(str::to_string).collect::<Vec<_>>();
            if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            }
        })
        .collect()
}

fn tool_color(color: ToolRgb) -> Color {
    Color::Rgb(color.0, color.1, color.2)
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

fn push_hard_wrapped_text(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    style: Style,
    fill: LineFill,
) {
    push_wrapped_text_with(lines, text, width, style, fill, wrap_line_hard);
}

fn push_wrapped_text_with(
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
        let len = text.chars().count();
        if len < width {
            text.push_str(&" ".repeat(width - len));
        }
    }
    Line::from(Span::styled(text, style))
}

fn padded_inner_width(width: usize) -> usize {
    width.saturating_sub(2).max(1)
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

fn styled_blank_line(width: usize, style: Style) -> Line<'static> {
    Line::from(Span::styled(" ".repeat(width.max(1)), style))
}

fn wrap_line_at_whitespace(line: &str, width: usize) -> Vec<String> {
    wrap_line_at_whitespace_ranges(line, width)
        .into_iter()
        .map(|range| line[range].to_string())
        .collect()
}

fn wrap_line_at_whitespace_ranges(line: &str, width: usize) -> Vec<std::ops::Range<usize>> {
    let width = width.max(1);
    if line.is_empty() {
        return std::iter::once(0..0).collect();
    }

    let mut ranges = Vec::new();
    let mut start = 0;
    while start < line.len() {
        let mut count = 0usize;
        let mut split_at_width = None;
        let mut whitespace_break = None;
        let mut saw_non_whitespace = false;
        let mut overflow = false;
        let mut prefer_width_split = false;

        for (relative_index, ch) in line[start..].char_indices() {
            if count == width {
                overflow = true;
                prefer_width_split = ch.is_whitespace();
                break;
            }

            count += 1;
            let next = start + relative_index + ch.len_utf8();
            if ch.is_whitespace() {
                if saw_non_whitespace {
                    whitespace_break = Some(next);
                }
            } else {
                saw_non_whitespace = true;
            }
            if count == width {
                split_at_width = Some(next);
            }
        }

        if !overflow {
            ranges.push(start..line.len());
            break;
        }

        let split = if prefer_width_split {
            split_at_width.expect("overflow requires a full-width split")
        } else {
            whitespace_break
                .filter(|split| *split > start)
                .unwrap_or_else(|| split_at_width.expect("overflow requires a full-width split"))
        };
        ranges.push(start..split);
        start = split;
    }

    ranges
}

fn wrap_line_hard(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        current.push(ch);
        if current.chars().count() >= width {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn complete_visual_prefix_preserves_trailing_newline_state() {
        assert_eq!(complete_visual_prefix_byte_index("a\n", 10), "a\n".len());
        assert_eq!(
            complete_visual_prefix_byte_index("a\n\n", 10),
            "a\n\n".len()
        );
        assert_eq!(complete_visual_prefix_byte_index("a\nb", 10), "a\n".len());
    }

    #[test]
    fn complete_visual_prefix_keeps_multibyte_boundaries() {
        assert_eq!(complete_visual_prefix_byte_index("éa", 2), "éa".len());
        assert_eq!(complete_visual_prefix_byte_index("éab", 2), "éa".len());
    }

    #[test]
    fn complete_visual_prefix_wraps_at_exact_width() {
        assert_eq!(complete_visual_prefix_byte_index("abc", 3), 3);
        assert_eq!(complete_visual_prefix_byte_index("abcd", 3), 3);
        assert_eq!(complete_visual_prefix_byte_index("abcdef", 3), 6);
    }

    #[test]
    fn wrapped_text_prefers_whitespace_boundaries() {
        let mut lines = Vec::new();
        push_wrapped_text(
            &mut lines,
            "hello wide world",
            10,
            Style::default(),
            LineFill::Natural,
        );

        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(
            rendered,
            vec!["hello wide".to_string(), " world".to_string()]
        );
    }

    #[test]
    fn complete_visual_prefix_prefers_whitespace_boundaries() {
        assert_eq!(
            complete_visual_prefix_byte_index("hello wide", 8),
            "hello ".len()
        );
        assert_eq!(
            complete_visual_prefix_byte_index("hello wide", 10),
            "hello wide".len()
        );
    }

    #[test]
    fn wrapped_text_preserves_leading_repeated_and_trailing_whitespace() {
        let mut lines = Vec::new();
        push_wrapped_text(
            &mut lines,
            "  indented\na  b\ntrail  ",
            20,
            Style::default(),
            LineFill::Natural,
        );

        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(
            rendered,
            vec![
                "  indented".to_string(),
                "a  b".to_string(),
                "trail  ".to_string()
            ]
        );
    }

    #[test]
    fn wrapped_text_preserves_tabs_and_whitespace_only_lines() {
        let mut lines = Vec::new();
        push_wrapped_text(
            &mut lines,
            "\tindented\n   ",
            20,
            Style::default(),
            LineFill::Natural,
        );

        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(rendered, vec!["\tindented".to_string(), "   ".to_string()]);
    }

    #[test]
    fn wrapped_text_preserves_whitespace_when_breaking_at_boundary() {
        let mut lines = Vec::new();
        push_wrapped_text(
            &mut lines,
            "hello   wide",
            8,
            Style::default(),
            LineFill::Natural,
        );

        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(rendered, vec!["hello   ".to_string(), "wide".to_string()]);
    }

    #[test]
    fn complete_visual_prefix_and_rendering_agree_on_whitespace_boundary() {
        let text = "hello   wide";
        let split = complete_visual_prefix_byte_index(text, 8);
        let mut lines = Vec::new();
        push_wrapped_text(&mut lines, text, 8, Style::default(), LineFill::Natural);

        assert_eq!(&text[..split], "hello   ");
        assert_eq!(line_text(&lines[0]), "hello   ");
    }

    #[test]
    fn complete_visual_prefix_and_rendering_agree_on_exact_width_trailing_space() {
        let text = "abc ";
        let split = complete_visual_prefix_byte_index(text, 3);
        let mut lines = Vec::new();
        push_wrapped_text(&mut lines, text, 3, Style::default(), LineFill::Natural);

        assert_eq!(&text[..split], "abc");
        assert_eq!(
            lines.iter().map(line_text).collect::<Vec<_>>(),
            vec!["abc".to_string(), " ".to_string()]
        );
    }

    #[test]
    fn long_words_still_hard_wrap() {
        let mut lines = Vec::new();
        push_wrapped_text(
            &mut lines,
            "abcdefghijk",
            5,
            Style::default(),
            LineFill::Natural,
        );

        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(
            rendered,
            vec!["abcde".to_string(), "fghij".to_string(), "k".to_string()]
        );
    }

    #[test]
    fn stream_fragment_rendering_preserves_blank_lines() {
        let mut lines = Vec::new();
        push_wrapped_text(&mut lines, "a\n\n", 10, Style::default(), LineFill::Natural);

        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(rendered, vec!["a".to_string(), String::new()]);
    }
}
