use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
};

use super::super::{
    overlay_panel::{
        clamp_panel_scroll, overlay_panel_inner_width, overlay_panel_layout, render_overlay_panel,
        OverlayPanelFrame,
    },
    render::{display_width, wrap_line_at_whitespace},
    theme::Theme,
};

pub(super) const TITLE: &str = "Side chat";
const FOOTER_IDLE: &str = "Enter send   Esc close";
const FOOTER_BUSY: &str = "Enter send   Esc cancel";
const INPUT_PREFIX: &str = "> ";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SideEntry {
    User(String),
    Assistant(String),
    Tool(String),
    Error(String),
}

#[derive(Debug)]
pub(super) struct SideComposer {
    text: String,
    cursor: usize,
}

impl SideComposer {
    fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
        }
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(super) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(super) fn take_text(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub(super) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        let index = self.byte_index(self.cursor);
        self.text.insert(index, ch);
        self.cursor += 1;
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        for ch in text.chars().filter(|ch| *ch != '\n' && *ch != '\r') {
            self.insert_char(ch);
        }
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_index(self.cursor);
        self.cursor -= 1;
        let start = self.byte_index(self.cursor);
        self.text.replace_range(start..end, "");
    }

    pub(super) fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(super) fn move_right(&mut self) {
        self.cursor = self.cursor.saturating_add(1).min(self.text.chars().count());
    }

    pub(super) fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn move_end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.text.len())
    }
}

#[derive(Debug)]
pub(super) struct SideOverlay {
    pub(super) entries: Vec<SideEntry>,
    pub(super) composer: SideComposer,
    pub(super) scroll: usize,
    pub(super) busy: bool,
    pub(super) snapshot: String,
    streaming_assistant: Option<String>,
}

impl SideOverlay {
    pub(super) fn new(snapshot: String) -> Self {
        Self {
            entries: Vec::new(),
            composer: SideComposer::new(),
            scroll: 0,
            busy: false,
            snapshot,
            streaming_assistant: None,
        }
    }

    pub(super) fn push_user(&mut self, text: String) {
        self.entries.push(SideEntry::User(text));
        self.follow_end();
    }

    pub(super) fn push_error(&mut self, text: String) {
        self.streaming_assistant = None;
        self.busy = false;
        self.entries.push(SideEntry::Error(text));
        self.follow_end();
    }

    pub(super) fn append_assistant_delta(&mut self, delta: &str) {
        match &mut self.streaming_assistant {
            Some(text) => text.push_str(delta),
            None => self.streaming_assistant = Some(delta.to_owned()),
        }
        self.follow_end();
    }

    pub(super) fn reset_assistant_stream(&mut self) {
        self.streaming_assistant = None;
    }

    pub(super) fn push_tool(&mut self, name: String) {
        self.entries.push(SideEntry::Tool(name));
        self.follow_end();
    }

    pub(super) fn finish_assistant(&mut self) {
        if let Some(text) = self.streaming_assistant.take() {
            if !text.is_empty() {
                self.entries.push(SideEntry::Assistant(text));
            }
        }
        self.busy = false;
        self.follow_end();
    }

    pub(super) fn mark_cancelled(&mut self) {
        if let Some(text) = self.streaming_assistant.take() {
            if !text.is_empty() {
                self.entries.push(SideEntry::Assistant(text));
            }
        }
        self.busy = false;
        self.follow_end();
    }

    fn follow_end(&mut self) {
        self.scroll = usize::MAX;
    }

    fn body_lines(&self, width: usize) -> Vec<Line<'static>> {
        let width = width.max(1);
        let mut lines = Vec::new();
        for entry in &self.entries {
            push_entry_lines(&mut lines, entry, width);
        }
        if let Some(text) = &self.streaming_assistant {
            push_wrapped(&mut lines, text, Theme::text(), width);
        } else if self.busy && self.streaming_assistant.is_none() {
            lines.push(Line::from(Span::styled("…", Theme::dim())));
        }
        lines
    }
}

fn push_entry_lines(lines: &mut Vec<Line<'static>>, entry: &SideEntry, width: usize) {
    match entry {
        SideEntry::User(text) => {
            lines.push(Line::from(Span::styled("you", Theme::accent())));
            push_wrapped(lines, text, Theme::text(), width);
        }
        SideEntry::Assistant(text) => {
            push_wrapped(lines, text, Theme::text(), width);
        }
        SideEntry::Tool(name) => {
            lines.push(Line::from(Span::styled(
                format!("tool {name}"),
                Theme::dim(),
            )));
        }
        SideEntry::Error(text) => {
            push_wrapped(lines, text, Theme::error(), width);
        }
    }
}

fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    style: ratatui::style::Style,
    width: usize,
) {
    if text.is_empty() {
        lines.push(Line::from(Span::styled(String::new(), style)));
        return;
    }
    for line in text.lines() {
        if line.is_empty() {
            lines.push(Line::from(Span::styled(String::new(), style)));
            continue;
        }
        for part in wrap_line_at_whitespace(line, width) {
            lines.push(Line::from(Span::styled(part.to_string(), style)));
        }
    }
}

pub(super) fn side_overlay_frame(overlay: &SideOverlay, area: Rect) -> Option<OverlayPanelFrame> {
    if area.width < 8 || area.height < 8 {
        return None;
    }
    let inner_width = overlay_panel_inner_width(area).max(1);
    let mut body = overlay.body_lines(inner_width);
    let input = format!("{INPUT_PREFIX}{}", overlay.composer.text());
    body.push(Line::from(Span::styled(
        "─".repeat(inner_width),
        Theme::dim(),
    )));
    body.push(Line::from(Span::styled(
        truncate_input(&input, inner_width),
        Theme::input_prompt(),
    )));

    let layout = overlay_panel_layout(area, body.len());
    let input_row = body.len().saturating_sub(1);
    let transcript_rows = layout.body_rows.saturating_sub(2);
    let max_scroll = input_row.saturating_sub(transcript_rows.saturating_add(1));
    let scroll = if overlay.scroll == usize::MAX {
        max_scroll
    } else {
        clamp_panel_scroll(overlay.scroll, input_row.saturating_sub(1), transcript_rows)
    };

    let footer = if overlay.busy {
        FOOTER_BUSY
    } else {
        FOOTER_IDLE
    };
    let mut frame = render_overlay_panel(TITLE, footer, &body, scroll, area);
    let cursor_x = INPUT_PREFIX
        .chars()
        .count()
        .saturating_add(overlay.composer.cursor())
        .min(inner_width.saturating_sub(1));
    let input_screen_row = layout
        .body_rows
        .saturating_sub(1)
        .min(input_row.saturating_sub(scroll));
    frame.cursor = Position {
        x: frame
            .outer
            .x
            .saturating_add(1)
            .saturating_add(cursor_x as u16),
        y: frame
            .outer
            .y
            .saturating_add(1)
            .saturating_add(input_screen_row as u16),
    };
    Some(frame)
}

fn truncate_input(input: &str, width: usize) -> String {
    if display_width(input) <= width {
        return input.to_string();
    }
    crate::tui::render::truncate_one_line(input, width)
}

impl SideOverlay {
    pub(super) fn scroll_by(&mut self, delta: isize, body_rows: usize, body_len: usize) {
        let transcript_rows = body_rows.saturating_sub(2);
        let max_scroll = body_len.saturating_sub(transcript_rows.saturating_add(1));
        let current = if self.scroll == usize::MAX {
            max_scroll
        } else {
            self.scroll.min(max_scroll)
        };
        self.scroll = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize).min(max_scroll)
        };
    }
}
