use std::path::Path;

use ratatui::{
    layout::Position,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use regex::RegexBuilder;

use super::{Entry, PickerItem, TuiInfo, UiPicker, INLINE_VIEWPORT_HEIGHT};

const MAX_PICKER_ITEMS: usize = INLINE_VIEWPORT_HEIGHT as usize - 3;

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

pub(super) fn session_header_lines(info: &TuiInfo, width: usize) -> Vec<Line<'static>> {
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
        Line::from(vec![
            Span::styled("provider", Style::default().fg(Color::DarkGray)),
            Span::raw(": "),
            Span::styled(info.provider.clone(), Style::default().fg(Color::Yellow)),
            Span::raw("  •  model: "),
            Span::styled(info.model.clone(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("cwd", Style::default().fg(Color::DarkGray)),
            Span::raw(": "),
            Span::styled(compact_cwd(&info.cwd), Style::default().fg(Color::Green)),
        ]),
        Line::from(vec![
            Span::styled("reasoning", Style::default().fg(Color::DarkGray)),
            Span::raw(": "),
            Span::styled(
                format!("effort {}", info.reasoning_effort),
                Style::default().fg(Color::Magenta),
            ),
            Span::raw("  •  summary: "),
            Span::styled(
                info.reasoning_summary.clone(),
                Style::default().fg(Color::Magenta),
            ),
        ]),
        Line::styled(divider, Style::default().fg(Color::DarkGray)),
        Line::raw(""),
    ]
}

pub(super) fn picker_lines(picker: &UiPicker, width: usize) -> Vec<Line<'static>> {
    let matching_indices = picker.matching_indices();
    let mut lines = Vec::with_capacity(matching_indices.len() + 2);
    let filter = if picker.filter.is_empty() {
        String::new()
    } else {
        format!("  filter: {}", picker.filter)
    };
    lines.push(styled_line(
        format!("{}  {}{}", picker.title, picker.help, filter),
        width,
        Style::default().fg(Color::DarkGray),
        LineFill::Natural,
    ));
    if matching_indices.is_empty() {
        lines.push(styled_line(
            "  no matches".to_string(),
            width,
            Style::default().fg(Color::DarkGray),
            LineFill::Natural,
        ));
        return lines;
    }

    let name_width = picker
        .items
        .iter()
        .map(|item| item.label.chars().count())
        .max()
        .unwrap_or(4)
        .max(4)
        .min(28)
        .min(width.saturating_sub(18).max(4));
    let description_width = width.saturating_sub(name_width + 6).max(1);
    lines.push(styled_line(
        format!("  {:<name_width$} | description", "name"),
        width,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
        LineFill::Natural,
    ));

    let start = visible_picker_match_start(picker, &matching_indices);
    for index in matching_indices
        .into_iter()
        .skip(start)
        .take(MAX_PICKER_ITEMS)
    {
        let item = &picker.items[index];
        let selected = index == picker.selected;
        let marker = if selected { ">" } else { " " };
        let label = truncate_one_line(&item.label, name_width);
        let description = truncate_one_line(&item.description, description_width);
        let text = format!("{marker} {label:<name_width$} | {description}");
        let style = if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        lines.push(styled_line(text, width, style, LineFill::Natural));
    }
    lines
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

pub(super) fn picker_matching_indices(items: &[PickerItem], filter: &str) -> Vec<usize> {
    let filter = filter.trim();
    if filter.is_empty() {
        return (0..items.len()).collect();
    }

    let Ok(regex) = RegexBuilder::new(filter).case_insensitive(true).build() else {
        return Vec::new();
    };

    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let haystack = format!("{} {} {}", item.label, item.value, item.description);
            regex.is_match(&haystack).then_some(index)
        })
        .collect()
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

fn compact_cwd(path: &Path) -> String {
    let Ok(home) = std::env::var("HOME") else {
        return path.display().to_string();
    };

    let home = Path::new(&home);
    if let Ok(rest) = path.strip_prefix(home) {
        let rel = rest.display().to_string();
        if rel.is_empty() {
            "~".to_string()
        } else {
            format!("~/{rel}")
        }
    } else {
        path.display().to_string()
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
        Entry::Tool {
            name,
            command,
            ok,
            content,
        } => push_tool_block(
            &mut lines,
            name,
            command.as_deref(),
            *ok,
            content,
            inner_width,
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
    name: &str,
    command: Option<&str>,
    ok: bool,
    content: &str,
    width: usize,
) {
    let style = if name == "skill" {
        if ok {
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(92, 80, 140))
        } else {
            Style::default().fg(Color::White).bg(Color::Rgb(95, 36, 36))
        }
    } else if matches!(name, "bash" | "read_file" | "write_file") {
        if ok {
            Style::default().fg(Color::White).bg(Color::Rgb(25, 75, 45))
        } else {
            Style::default().fg(Color::White).bg(Color::Rgb(95, 36, 36))
        }
    } else {
        Style::default()
            .fg(Color::Yellow)
            .bg(Color::Rgb(48, 45, 30))
    };

    push_wrapped_text(lines, name, width, style, LineFill::PadToWidth);
    if name == "bash" {
        if let Some(command) = command.filter(|command| !command.trim().is_empty()) {
            push_wrapped_text(lines, command, width, style, LineFill::PadToWidth);
        }
        if !content.trim().is_empty() {
            push_wrapped_text(lines, content, width, style, LineFill::PadToWidth);
        }
    } else if !content.trim().is_empty() {
        push_wrapped_text(lines, content, width, style, LineFill::PadToWidth);
    }
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
