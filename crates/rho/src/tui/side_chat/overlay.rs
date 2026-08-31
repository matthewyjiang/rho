use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
};

use super::super::{
    line_editor::LineEditor,
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

pub(super) struct SideScrollMetrics {
    pub(super) body_len: usize,
    pub(super) body_rows: usize,
    pub(super) max_scroll: usize,
}

#[derive(Debug)]
pub(super) struct SideOverlay {
    pub(super) entries: Vec<SideEntry>,
    pub(super) composer: LineEditor,
    pub(super) scroll: usize,
    pub(super) busy: bool,
    pub(super) snapshot: String,
    streaming_assistant: Option<String>,
}

impl SideOverlay {
    pub(super) fn new(snapshot: String) -> Self {
        Self {
            entries: Vec::new(),
            composer: LineEditor::new(""),
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

    pub(super) fn push_notice(&mut self, text: String) {
        self.entries.push(SideEntry::Error(text));
        self.follow_end();
    }

    pub(super) fn fail_run(&mut self, text: String) {
        self.commit_stream();
        self.busy = false;
        self.push_notice(text);
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
        self.commit_stream();
        self.busy = false;
        self.follow_end();
    }

    pub(super) fn mark_cancelled(&mut self) {
        self.commit_stream();
        self.busy = false;
        self.follow_end();
    }

    fn commit_stream(&mut self) {
        if let Some(text) = self.streaming_assistant.take() {
            if !text.is_empty() {
                self.entries.push(SideEntry::Assistant(text));
            }
        }
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
        } else if self.busy {
            lines.push(Line::from(Span::styled("…", Theme::dim())));
        }
        lines
    }

    pub(super) fn scroll_by(&mut self, delta: isize, metrics: &SideScrollMetrics) {
        let current = resolve_side_scroll(self.scroll, metrics);
        self.scroll = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current
                .saturating_add(delta as usize)
                .min(metrics.max_scroll)
        };
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

pub(super) fn side_overlay_panel_body(
    overlay: &SideOverlay,
    inner_width: usize,
) -> Vec<Line<'static>> {
    let mut body = overlay.body_lines(inner_width);
    body.push(Line::from(Span::styled(
        "─".repeat(inner_width),
        Theme::dim(),
    )));
    let input = format!("{INPUT_PREFIX}{}", overlay.composer.value);
    body.push(Line::from(Span::styled(
        truncate_input(&input, inner_width),
        Theme::input_prompt(),
    )));
    body
}

pub(super) fn side_scroll_metrics(overlay: &SideOverlay, area: Rect) -> Option<SideScrollMetrics> {
    if area.width < 8 || area.height < 8 {
        return None;
    }
    let inner_width = overlay_panel_inner_width(area).max(1);
    let body_len = side_overlay_panel_body(overlay, inner_width).len();
    let body_rows = overlay_panel_layout(area, body_len).body_rows;
    Some(SideScrollMetrics {
        body_len,
        body_rows,
        max_scroll: side_max_scroll(body_len, body_rows),
    })
}

fn side_max_scroll(body_len: usize, body_rows: usize) -> usize {
    let input_row = body_len.saturating_sub(1);
    let transcript_rows = body_rows.saturating_sub(2);
    input_row.saturating_sub(transcript_rows.saturating_add(1))
}

fn resolve_side_scroll(scroll: usize, metrics: &SideScrollMetrics) -> usize {
    if scroll == usize::MAX {
        metrics.max_scroll
    } else {
        let input_row = metrics.body_len.saturating_sub(1);
        let transcript_rows = metrics.body_rows.saturating_sub(2);
        clamp_panel_scroll(scroll, input_row.saturating_sub(1), transcript_rows)
    }
}

pub(super) fn side_overlay_frame(overlay: &SideOverlay, area: Rect) -> Option<OverlayPanelFrame> {
    let metrics = side_scroll_metrics(overlay, area)?;
    let inner_width = overlay_panel_inner_width(area).max(1);
    let body = side_overlay_panel_body(overlay, inner_width);
    let scroll = resolve_side_scroll(overlay.scroll, &metrics);
    let input_row = body.len().saturating_sub(1);

    let footer = if overlay.busy {
        FOOTER_BUSY
    } else {
        FOOTER_IDLE
    };
    let mut frame = render_overlay_panel(TITLE, footer, &body, scroll, area);
    let cursor_x = INPUT_PREFIX
        .chars()
        .count()
        .saturating_add(overlay.composer.cursor)
        .min(inner_width.saturating_sub(1));
    let input_screen_row = metrics
        .body_rows
        .saturating_sub(1)
        .min(input_row.saturating_sub(scroll));
    frame.cursor = Some(Position {
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
    });
    Some(frame)
}

fn truncate_input(input: &str, width: usize) -> String {
    if display_width(input) <= width {
        return input.to_string();
    }
    crate::tui::render::truncate_one_line(input, width)
}

#[cfg(test)]
#[path = "overlay_tests.rs"]
mod tests;
