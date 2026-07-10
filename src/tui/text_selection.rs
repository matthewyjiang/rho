use std::{
    io,
    ops::Range,
    time::{Duration, Instant},
};

use crossterm::{clipboard::CopyToClipboard, execute};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::theme::Theme;

const COPY_NOTICE_DURATION: Duration = Duration::from_secs(2);

pub(super) trait ClipboardWriter {
    fn copy(&mut self, text: &str) -> io::Result<()>;
}

#[derive(Default)]
pub(super) struct TerminalClipboard;

impl ClipboardWriter for TerminalClipboard {
    fn copy(&mut self, text: &str) -> io::Result<()> {
        let mut stdout = io::stdout();
        execute!(stdout, CopyToClipboard::to_clipboard_from(text))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SelectionPosition {
    pub(super) line: usize,
    pub(super) column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TextSelection {
    anchor: SelectionPosition,
    focus: SelectionPosition,
}

impl TextSelection {
    pub(super) fn new(position: SelectionPosition) -> Self {
        Self {
            anchor: position,
            focus: position,
        }
    }

    pub(super) fn update(&mut self, position: SelectionPosition) {
        self.focus = position;
    }

    pub(super) fn has_moved(self) -> bool {
        self.anchor != self.focus
    }

    pub(super) fn selected_text(self, lines: &[Line<'_>], first_line: usize) -> Option<String> {
        if !self.has_moved() {
            return None;
        }

        let (start, end) = self.ordered_positions();
        let mut selected = Vec::with_capacity(end.line.saturating_sub(start.line) + 1);
        for line_index in start.line..=end.line {
            let line = lines.get(line_index.checked_sub(first_line)?)?;
            let text = rendered_line_text(line);
            let start_column = if line_index == start.line {
                start.column
            } else {
                0
            };
            let end_column = if line_index == end.line {
                end.column.saturating_add(1)
            } else {
                usize::MAX
            };
            selected.push(
                text_for_display_columns(&text, start_column..end_column)
                    .trim_end_matches(' ')
                    .to_string(),
            );
        }

        let text = selected.join("\n");
        (!text.is_empty()).then_some(text)
    }

    fn selected_columns(self, line: usize) -> Option<Range<usize>> {
        if !self.has_moved() {
            return None;
        }
        let (start, end) = self.ordered_positions();
        if !(start.line..=end.line).contains(&line) {
            return None;
        }

        let start_column = if line == start.line { start.column } else { 0 };
        let end_column = if line == end.line {
            end.column.saturating_add(1)
        } else {
            usize::MAX
        };
        Some(start_column..end_column)
    }

    fn ordered_positions(self) -> (SelectionPosition, SelectionPosition) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
}

pub(super) fn highlight_selection(
    buffer: &mut Buffer,
    area: Rect,
    first_visible_line: usize,
    selection: TextSelection,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    for row_offset in 0..area.height as usize {
        let line = first_visible_line.saturating_add(row_offset);
        let Some(columns) = selection.selected_columns(line) else {
            continue;
        };
        let start = columns.start.min(area.width as usize);
        let end = columns.end.min(area.width as usize);
        for column_offset in start..end {
            buffer[(
                area.x.saturating_add(column_offset as u16),
                area.y.saturating_add(row_offset as u16),
            )]
                .set_style(Style::default().add_modifier(Modifier::REVERSED));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyNoticeTone {
    Success,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CopyNotice {
    message: String,
    tone: CopyNoticeTone,
    visible_until: Instant,
}

impl CopyNotice {
    pub(super) fn copied(character_count: usize, now: Instant) -> Self {
        let unit = if character_count == 1 {
            "char"
        } else {
            "chars"
        };
        Self {
            message: format!("{character_count} {unit} copied"),
            tone: CopyNoticeTone::Success,
            visible_until: now + COPY_NOTICE_DURATION,
        }
    }

    pub(super) fn failed(error: &io::Error, now: Instant) -> Self {
        Self {
            message: format!("copy failed: {error}"),
            tone: CopyNoticeTone::Error,
            visible_until: now + COPY_NOTICE_DURATION,
        }
    }

    pub(super) fn visible_until(&self) -> Instant {
        self.visible_until
    }

    pub(super) fn is_visible(&self, now: Instant) -> bool {
        now < self.visible_until
    }

    #[cfg(test)]
    pub(super) fn message(&self) -> &str {
        &self.message
    }
}

pub(super) fn render_copy_notice(
    frame: &mut Frame<'_>,
    area: Rect,
    notice: &CopyNotice,
    now: Instant,
) {
    if !notice.is_visible(now) || area.width == 0 || area.height == 0 {
        return;
    }

    let popup_width = UnicodeWidthStr::width(notice.message.as_str())
        .saturating_add(4)
        .min(area.width as usize) as u16;
    let popup_height = area.height.min(3);
    let popup = Rect::new(
        area.x
            .saturating_add(area.width.saturating_sub(popup_width)),
        area.y,
        popup_width,
        popup_height,
    );
    let style = match notice.tone {
        CopyNoticeTone::Success => Theme::success(),
        CopyNoticeTone::Error => Theme::error(),
    };

    frame.render_widget(Clear, popup);
    if popup.width >= 3 && popup.height >= 3 {
        frame.render_widget(
            Paragraph::new(notice.message.as_str())
                .alignment(Alignment::Center)
                .style(style)
                .block(Block::default().borders(Borders::ALL).border_style(style)),
            popup,
        );
    } else {
        frame.render_widget(
            Paragraph::new(notice.message.as_str())
                .alignment(Alignment::Right)
                .style(style),
            popup,
        );
    }
}

fn rendered_line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .filter(|character| !character.is_control())
        .collect()
}

fn text_for_display_columns(text: &str, columns: Range<usize>) -> String {
    let mut column: usize = 0;
    let mut selected = String::new();
    for grapheme in text.graphemes(true) {
        let width = UnicodeWidthStr::width(grapheme);
        let next_column = column.saturating_add(width);
        if next_column > columns.start && column < columns.end {
            selected.push_str(grapheme);
        }
        column = next_column;
        if column >= columns.end {
            break;
        }
    }
    selected
}

#[cfg(test)]
#[path = "text_selection_tests.rs"]
mod tests;
