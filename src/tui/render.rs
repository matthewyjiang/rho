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
        lines.push(picker_item_line(item, selected, label_width));
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
    picker
        .items
        .iter()
        .map(|item| item.label.chars().count())
        .max()
        .unwrap_or(12)
        .clamp(12, 30)
        .min(width.saturating_sub(18).max(12))
}

fn picker_item_line(item: &PickerItem, selected: bool, label_width: usize) -> Line<'static> {
    let marker = if selected { "→" } else { " " };
    let row_style = if selected {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::White)
    };
    let label = truncate_one_line(&item.label, label_width);
    let mut spans = vec![Span::styled(
        format!("{marker} {label:<label_width$}"),
        row_style,
    )];
    if let Some(badge) = &item.badge {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            truncate_one_line(&badge.text, 24),
            picker_badge_style(badge.tone),
        ));
    }
    Line::from(spans)
}

fn picker_footer_text(picker: &UiPicker) -> String {
    let action = match picker.action {
        super::PickerAction::Config => "change",
        super::PickerAction::SelectModel
        | super::PickerAction::LoginProvider
        | super::PickerAction::LogoutProvider
        | super::PickerAction::InsertSkillCommand => "select",
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

pub(super) fn byte_index_after_visual_lines(
    text: &str,
    width: usize,
    target_lines: usize,
) -> Option<usize> {
    if target_lines == 0 {
        return Some(0);
    }

    let width = width.max(1);
    let mut completed = 0;
    let mut column = 0;
    for (index, ch) in text.char_indices() {
        let next = index + ch.len_utf8();
        if ch == '\n' {
            completed += 1;
            column = 0;
        } else {
            column += 1;
            if column >= width {
                completed += 1;
                column = 0;
            }
        }

        if completed >= target_lines {
            return Some(next);
        }
    }
    None
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
        let wrapped = wrap_line(raw_line, width);
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

pub(super) fn entry_lines(entry: &Entry, width: usize) -> Vec<Line<'static>> {
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
            ..
        } => push_tool_block(&mut lines, *ok, display_lines, *display_style, inner_width),
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
) {
    let background = if ok {
        display_style.success_background
    } else {
        display_style.failure_background
    };
    let style = Style::default()
        .fg(tool_color(display_style.foreground))
        .bg(tool_color(background));

    for line in display_lines {
        push_wrapped_text(lines, line, width, style, LineFill::PadToWidth);
    }
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

fn wrap_line(line: &str, width: usize) -> Vec<String> {
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
