//! Selected-node details pane: durable output body + document scroll.

use std::{path::PathBuf, time::Instant};

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
};

use super::super::{
    scrollbar::{HistoryScrollChrome, HistoryScrollbar, ScrollbarMouseInput},
    theme::Theme,
    HISTORY_MOUSE_SCROLL_LINES, HISTORY_SCROLLBAR_REVEAL_DURATION,
};
use super::{
    event_adapter::WorkflowNodeSnapshot,
    output::{self, NodeOutputBody},
};

/// Right-pane output viewer for a finished workflow node.
#[derive(Debug, Default)]
pub(super) struct DetailPane {
    run_directory: Option<PathBuf>,
    body: Option<NodeOutputBody>,
    scroll: HistoryScrollChrome,
    area: Rect,
    content_len: usize,
    viewport: usize,
    cached_width: Option<usize>,
    cached_lines: Vec<Line<'static>>,
}

impl DetailPane {
    pub(super) fn set_run_directory(&mut self, run_directory: Option<PathBuf>) {
        self.run_directory = run_directory;
        self.clear_body(/*reset_scroll*/ true);
    }

    pub(super) fn body(&self) -> Option<&NodeOutputBody> {
        self.body.as_ref()
    }

    pub(super) fn has_body(&self) -> bool {
        self.body.is_some()
    }

    pub(super) fn is_scrollable(&self) -> bool {
        self.content_len > self.viewport && self.viewport > 0
    }

    pub(super) fn should_render_scrollbar(&self, now: Instant) -> bool {
        self.scroll.should_render(now)
    }

    pub(super) fn dragging_scrollbar(&self) -> bool {
        self.scroll.drag().is_some()
    }

    /// Load durable output for the selected node when the cache is stale.
    pub(super) fn refresh(&mut self, node: Option<&WorkflowNodeSnapshot>, reset_scroll: bool) {
        let Some(node) = node else {
            self.clear_body(reset_scroll);
            return;
        };
        let Some(run_directory) = self.run_directory.as_ref() else {
            self.clear_body(reset_scroll);
            return;
        };
        if self
            .body
            .as_ref()
            .is_some_and(|body| output::body_matches_node(body, node))
        {
            return;
        }
        self.body = output::load_finished_output(run_directory, node);
        self.invalidate_line_cache();
        if reset_scroll {
            self.scroll = HistoryScrollChrome::default();
            // Stay top-anchored even before geometry is known.
            self.scroll.pin_top_line(0, 0, 0);
        }
    }

    /// Ensure body lines are rendered for `width` and return the full line count.
    pub(super) fn prepare_body_lines(&mut self, width: usize) -> usize {
        let width = width.max(1);
        if self.cached_width != Some(width) {
            self.cached_width = Some(width);
            self.cached_lines = match self.body.as_ref() {
                Some(body) => {
                    let mut lines = vec![
                        Line::from(Span::styled(
                            format!("{} · {}", output::kind_label(body.kind), body.relative_path),
                            Theme::dim(),
                        )),
                        Line::styled("─".repeat(width), Theme::dim()),
                    ];
                    lines.extend(output::render_body_lines(body, width));
                    lines
                }
                None => Vec::new(),
            };
        }
        self.cached_lines.len()
    }

    /// Clone only the currently visible window for rendering.
    pub(super) fn visible_body_lines(&self) -> Vec<Line<'static>> {
        if self.cached_lines.is_empty() || self.viewport == 0 {
            return Vec::new();
        }
        let start = self.visible_start().min(self.cached_lines.len());
        let end = start
            .saturating_add(self.viewport)
            .min(self.cached_lines.len());
        self.cached_lines[start..end].to_vec()
    }

    pub(super) fn sync_geometry(&mut self, area: Rect, content_len: usize, viewport_len: usize) {
        self.area = area;
        self.content_len = content_len;
        self.viewport = viewport_len;
        let top = self.scroll.visible_start(content_len, viewport_len);
        self.scroll.pin_top_line(content_len, viewport_len, top);
    }

    pub(super) fn visible_start(&self) -> usize {
        self.scroll.visible_start(self.content_len, self.viewport)
    }

    pub(super) fn scroll_by(&mut self, delta: isize) {
        if self.viewport == 0 {
            return;
        }
        let max_start = self.content_len.saturating_sub(self.viewport);
        let next = self
            .visible_start()
            .saturating_add_signed(delta)
            .min(max_start);
        self.scroll
            .pin_top_line(self.content_len, self.viewport, next);
        self.scroll.set_drag(None);
    }

    pub(super) fn scroll_page(&mut self, direction: isize) {
        let page = self.viewport.max(1) as isize;
        self.scroll_by(direction.saturating_mul(page));
    }

    pub(super) fn scroll_home(&mut self) {
        self.scroll.pin_top_line(self.content_len, self.viewport, 0);
        self.scroll.set_drag(None);
    }

    pub(super) fn scroll_end(&mut self) {
        self.scroll.scroll_to_bottom();
    }

    pub(super) fn reveal_scrollbar(&mut self, now: Instant) {
        if self.is_scrollable() {
            self.scroll.reveal(now, HISTORY_SCROLLBAR_REVEAL_DURATION);
        }
    }

    pub(super) fn handle_mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) -> bool {
        if self.area.width == 0 || self.area.height == 0 {
            return false;
        }
        let now = Instant::now();
        let scrollbar = self.scrollbar();
        let over_details = self.area.contains((column, row).into())
            || scrollbar.is_some_and(|bar| bar.contains(column, row));
        let dragging = matches!(
            kind,
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        );
        if !over_details && !dragging {
            return false;
        }

        let before = (
            self.visible_start(),
            self.scroll.drag().is_some(),
            self.scroll.hovered(),
            self.scroll.should_render(now),
        );
        self.scroll.handle_scrollbar_mouse(
            kind,
            column,
            row,
            ScrollbarMouseInput {
                now,
                reveal_duration: HISTORY_SCROLLBAR_REVEAL_DURATION,
                scrollbar,
                content_len: self.content_len,
                viewport_len: self.viewport,
                wheel_lines: HISTORY_MOUSE_SCROLL_LINES,
            },
        );
        // Mouse helpers may collapse top into Bottom stickiness; re-pin document top.
        let top = self.visible_start();
        self.scroll
            .pin_top_line(self.content_len, self.viewport, top);
        let after = (
            self.visible_start(),
            self.scroll.drag().is_some(),
            self.scroll.hovered(),
            self.scroll.should_render(now),
        );
        before != after
    }

    pub(super) fn scrollbar(&self) -> Option<HistoryScrollbar> {
        HistoryScrollbar::new(self.area, self.content_len, self.visible_start())
    }

    fn clear_body(&mut self, reset_scroll: bool) {
        self.body = None;
        self.invalidate_line_cache();
        if reset_scroll {
            self.scroll = HistoryScrollChrome::default();
        }
    }

    fn invalidate_line_cache(&mut self) {
        self.cached_width = None;
        self.cached_lines.clear();
    }
}

#[cfg(test)]
#[path = "details_tests.rs"]
mod tests;
