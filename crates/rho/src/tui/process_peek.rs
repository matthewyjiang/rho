//! In-place read-only peek view for activity-rail process rows.

use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::{
    exclusive_screen::ExclusiveOccupant,
    process_panel::{self, ProcessPeekTarget},
    render::{truncate_one_line, wrap_line_hard},
    scrollbar::{HistoryScrollChrome, HistoryScrollbar, ScrollbarMouseInput},
    theme::Theme,
    App, HistoryScroll, HISTORY_MOUSE_SCROLL_LINES, HISTORY_SCROLLBAR_REVEAL_DURATION,
};
use crate::{
    subagent,
    tools::process::{Chunk, HostProcessView, ProcessManager, Stream},
};

const FOOTER_HINT: &str = "read-only · scroll · q back · ctrl+c back/again quit";
const EVICTED_NOTICE: &str = "earlier output evicted";
const STDERR_TAG: &str = "stderr ";

/// Result of routing one terminal event through the peek view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PeekInput {
    Ignored,
    Handled,
    Leave,
    Quit,
}

/// One logical output line before wrap, or the eviction notice.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PeekBodyLine {
    Evicted,
    Output { stream: Stream, text: String },
}

pub(super) struct ProcessPeekView {
    process_id: String,
    view: HostProcessView,
    manager: ProcessManager,
    scroll: HistoryScrollChrome,
    last_drawn_elapsed_secs: Option<u64>,
    viewport_height: usize,
    history_area: Rect,
    content_len: usize,
}

impl ProcessPeekView {
    fn new(view: HostProcessView, manager: ProcessManager) -> Self {
        Self {
            process_id: view.snapshot.process_id.clone(),
            view,
            manager,
            scroll: HistoryScrollChrome::default(),
            last_drawn_elapsed_secs: None,
            viewport_height: 0,
            history_area: Rect::default(),
            content_len: 0,
        }
    }

    pub(super) fn should_redraw(&self, now: Instant) -> bool {
        self.scroll.should_render(now) || self.live_elapsed_secs() != self.last_drawn_elapsed_secs
    }

    fn note_drawn(&mut self) {
        self.last_drawn_elapsed_secs = self.live_elapsed_secs();
    }

    fn live_elapsed_secs(&self) -> Option<u64> {
        self.view
            .snapshot
            .state
            .is_live()
            .then_some(self.view.elapsed_seconds)
    }

    pub(super) fn refresh(&mut self) -> bool {
        let Ok(next) = self.manager.host_view(&self.process_id) else {
            return false;
        };
        if !view_changed(&self.view, &next) {
            return false;
        }
        self.view = next;
        true
    }

    fn handle_event(&mut self, event: Event) -> PeekInput {
        let now = Instant::now();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    PeekInput::Quit
                }
                KeyCode::Char('q') | KeyCode::Esc => PeekInput::Leave,
                KeyCode::Up => {
                    self.scroll_lines(now, -1);
                    PeekInput::Handled
                }
                KeyCode::Down => {
                    self.scroll_lines(now, 1);
                    PeekInput::Handled
                }
                KeyCode::PageUp => {
                    self.scroll_lines(now, -(self.viewport_height.max(1) as isize));
                    PeekInput::Handled
                }
                KeyCode::PageDown => {
                    self.scroll_lines(now, self.viewport_height.max(1) as isize);
                    PeekInput::Handled
                }
                KeyCode::Home => {
                    self.scroll
                        .set_top_line(self.content_len, self.viewport_height, 0);
                    if !matches!(self.scroll.scroll(), HistoryScroll::Bottom) {
                        self.scroll.reveal(now, HISTORY_SCROLLBAR_REVEAL_DURATION);
                    }
                    PeekInput::Handled
                }
                KeyCode::End => {
                    self.scroll.scroll_to_bottom();
                    PeekInput::Handled
                }
                _ => PeekInput::Ignored,
            },
            Event::Mouse(mouse) => {
                self.scroll.handle_scrollbar_mouse(
                    mouse.kind,
                    mouse.column,
                    mouse.row,
                    ScrollbarMouseInput {
                        now,
                        reveal_duration: HISTORY_SCROLLBAR_REVEAL_DURATION,
                        scrollbar: self.history_scrollbar(),
                        content_len: self.content_len,
                        viewport_len: self.viewport_height,
                        wheel_lines: HISTORY_MOUSE_SCROLL_LINES,
                    },
                );
                match mouse.kind {
                    MouseEventKind::Moved => PeekInput::Ignored,
                    _ => PeekInput::Handled,
                }
            }
            Event::Resize(_, _) => PeekInput::Handled,
            _ => PeekInput::Ignored,
        }
    }

    fn scroll_lines(&mut self, now: Instant, delta: isize) {
        self.scroll
            .scroll_by(self.content_len, self.viewport_height, delta);
        if !matches!(self.scroll.scroll(), HistoryScroll::Bottom) {
            self.scroll.reveal(now, HISTORY_SCROLLBAR_REVEAL_DURATION);
        }
    }

    fn history_scrollbar(&self) -> Option<HistoryScrollbar> {
        HistoryScrollbar::new(
            self.history_area,
            self.content_len,
            self.scroll
                .visible_start(self.content_len, self.viewport_height),
        )
    }

    fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);
        let width = area.width as usize;
        let header = peek_header_lines(&self.view, width);
        frame.render_widget(Paragraph::new(header), chunks[0]);

        let body = peek_output_lines(
            &self.view.snapshot.chunks,
            self.view.snapshot.truncated,
            width,
        );
        self.history_area = chunks[1];
        self.viewport_height = chunks[1].height as usize;
        self.content_len = body.len();
        self.scroll.clamp(self.content_len, self.viewport_height);
        let start = self
            .scroll
            .visible_start(self.content_len, self.viewport_height);
        let end = start.saturating_add(self.viewport_height).min(body.len());
        frame.render_widget(Paragraph::new(body[start..end].to_vec()), chunks[1]);

        let now = Instant::now();
        if let Some(scrollbar) = self
            .history_scrollbar()
            .filter(|_| self.scroll.should_render(now))
        {
            scrollbar.render(frame, self.scroll.drag().is_some());
        }

        let footer = vec![
            Line::styled("─".repeat(width.max(1)), Theme::dim()),
            Line::styled(truncate_one_line(FOOTER_HINT, width), Theme::dim()),
        ];
        frame.render_widget(Paragraph::new(footer).style(Style::default()), chunks[2]);
    }
}

impl App {
    pub(super) fn draw_peek_screen(&mut self, frame: &mut Frame<'_>) -> bool {
        let Some(view) = self.exclusive.peek_view_mut() else {
            return false;
        };
        view.draw(frame);
        view.note_drawn();
        true
    }

    pub(super) fn activate_process_row(&mut self, target: &ProcessPeekTarget) {
        if let Err(error) = self.enter_peek_view(&target.process_id) {
            self.notify_status(format!("could not peek process: {error}"));
        }
    }

    pub(super) fn enter_peek_view(&mut self, process_id: &str) -> anyhow::Result<()> {
        let view = self.process_panel.host_view(process_id)?;
        let manager = self
            .process_panel
            .manager()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("process manager unavailable"))?;
        let command = process_panel::command_identity(&view.snapshot.command).to_owned();
        self.exclusive = ExclusiveOccupant::Peek {
            view: Box::new(ProcessPeekView::new(view, manager)),
        };
        self.notify_status(format!("peeking {command}"));
        Ok(())
    }

    pub(super) fn leave_peek_view(&mut self) {
        if matches!(self.exclusive, ExclusiveOccupant::Peek { .. }) {
            self.exclusive = ExclusiveOccupant::Session;
        }
    }

    pub(super) fn route_peek_event(&mut self, event: Event) -> bool {
        let resize = matches!(event, Event::Resize(_, _));
        let Some(view) = self.exclusive.peek_view_mut() else {
            return resize;
        };
        match view.handle_event(event) {
            PeekInput::Leave => self.leave_peek_view(),
            PeekInput::Quit => {
                self.leave_peek_view();
                self.notify_status("left peek view; press ctrl-c again to quit");
                self.ctrl_c_streak = 1;
            }
            PeekInput::Ignored | PeekInput::Handled => {}
        }
        resize
    }
}

fn view_changed(previous: &HostProcessView, next: &HostProcessView) -> bool {
    previous.elapsed_seconds != next.elapsed_seconds
        || previous.quiet_seconds != next.quiet_seconds
        || previous.snapshot.state != next.snapshot.state
        || previous.snapshot.exit_code != next.snapshot.exit_code
        || previous.snapshot.truncated != next.snapshot.truncated
        || previous.snapshot.first_cursor != next.snapshot.first_cursor
        || previous.snapshot.next_cursor != next.snapshot.next_cursor
        || previous.snapshot.available_cursor != next.snapshot.available_cursor
        || previous.snapshot.command != next.snapshot.command
        || previous.snapshot.chunks != next.snapshot.chunks
}

fn peek_header_lines(view: &HostProcessView, width: usize) -> Vec<Line<'static>> {
    let command = process_panel::command_identity(&view.snapshot.command);
    let (status, status_style) = process_panel::process_activity_for(
        view.snapshot.state,
        view.quiet_seconds,
        view.snapshot.exit_code,
    );
    let mut meta = vec![
        Span::styled(status, status_style),
        Span::styled(
            format!(" · {}", subagent::format_elapsed_secs(view.elapsed_seconds)),
            Theme::dim(),
        ),
    ];
    if let Some(quiet) = view
        .quiet_seconds
        .filter(|quiet| *quiet < process_panel::QUIET_LABEL_AFTER)
    {
        meta.push(Span::styled(
            format!(" · quiet {}", subagent::format_elapsed_secs(quiet)),
            Theme::dim(),
        ));
    }
    vec![
        Line::from(vec![
            Span::styled(super::activity::PROCESS_GLYPH, Theme::text_strong()),
            Span::styled(
                truncate_one_line(command, width.saturating_sub(2)),
                Theme::text_strong(),
            ),
        ]),
        Line::from(meta),
        Line::styled("─".repeat(width.max(1)), Theme::dim()),
    ]
}

fn peek_body_model(chunks: &[Chunk], truncated: bool) -> Vec<PeekBodyLine> {
    let mut lines = Vec::new();
    if truncated {
        lines.push(PeekBodyLine::Evicted);
    }
    let mut pending: Option<(Stream, String)> = None;
    for chunk in chunks {
        match pending.as_mut() {
            Some((stream, text))
                if *stream == chunk.stream && !text.ends_with('\n') && !text.ends_with('\r') =>
            {
                text.push_str(&chunk.text);
            }
            _ => {
                flush_pending_chunk(&mut lines, pending.take());
                pending = Some((chunk.stream, chunk.text.clone()));
            }
        }
    }
    flush_pending_chunk(&mut lines, pending);
    lines
}

fn flush_pending_chunk(lines: &mut Vec<PeekBodyLine>, pending: Option<(Stream, String)>) {
    let Some((stream, text)) = pending else {
        return;
    };
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']).to_owned();
        if line.is_empty() && !raw.ends_with('\n') && !raw.ends_with('\r') {
            continue;
        }
        lines.push(PeekBodyLine::Output { stream, text: line });
    }
}

fn peek_output_lines(chunks: &[Chunk], truncated: bool, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for item in peek_body_model(chunks, truncated) {
        match item {
            PeekBodyLine::Evicted => {
                lines.push(Line::styled(
                    truncate_one_line(EVICTED_NOTICE, width),
                    Theme::dim(),
                ));
            }
            PeekBodyLine::Output { stream, text } => {
                lines.extend(render_output_line(stream, &text, width));
            }
        }
    }
    lines
}

fn render_output_line(stream: Stream, text: &str, width: usize) -> Vec<Line<'static>> {
    match stream {
        Stream::Stdout => wrap_line_hard(text, width)
            .into_iter()
            .map(|part| Line::raw(part.to_owned()))
            .collect(),
        Stream::Stderr => {
            let tag_width = super::render::display_width(STDERR_TAG);
            let body_width = width.saturating_sub(tag_width).max(1);
            wrap_line_hard(text, body_width)
                .into_iter()
                .enumerate()
                .map(|(index, part)| {
                    if index == 0 {
                        Line::from(vec![
                            Span::styled(STDERR_TAG, Theme::dim()),
                            Span::raw(part.to_owned()),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw(" ".repeat(tag_width)),
                            Span::raw(part.to_owned()),
                        ])
                    }
                })
                .collect()
        }
    }
}

#[cfg(test)]
#[path = "process_peek_tests.rs"]
mod tests;
