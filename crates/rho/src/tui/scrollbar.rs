use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::{layout::Rect, style::Modifier, Frame};

use super::{theme::Theme, HistoryScroll};

/// Shared history scroll + auto-hide scrollbar interaction state.
///
/// Owned by the main transcript UI and the read-only attach view so both paths
/// share one reveal/drag/hover/clamp policy.
#[derive(Clone, Debug, Default)]
pub(super) struct HistoryScrollChrome {
    scroll: HistoryScroll,
    drag: Option<HistoryScrollbarDrag>,
    visible_until: Option<Instant>,
    hovered: bool,
}

impl HistoryScrollChrome {
    pub(super) fn scroll(&self) -> HistoryScroll {
        self.scroll
    }

    pub(super) fn drag(&self) -> Option<HistoryScrollbarDrag> {
        self.drag
    }

    pub(super) fn set_drag(&mut self, drag: Option<HistoryScrollbarDrag>) {
        self.drag = drag;
    }

    pub(super) fn hovered(&self) -> bool {
        self.hovered
    }

    pub(super) fn visible_until(&self) -> Option<Instant> {
        self.visible_until
    }

    pub(super) fn reveal(&mut self, now: Instant, duration: Duration) {
        self.visible_until = Some(now + duration);
    }

    pub(super) fn hide(&mut self) {
        self.drag = None;
        self.visible_until = None;
        self.hovered = false;
    }

    pub(super) fn should_render(&self, now: Instant) -> bool {
        self.drag.is_some()
            || self.hovered
            || self
                .visible_until
                .is_some_and(|visible_until| now < visible_until)
    }

    pub(super) fn visible_start(&self, content_len: usize, viewport_len: usize) -> usize {
        let max_start = content_len.saturating_sub(viewport_len);
        match self.scroll {
            HistoryScroll::Bottom => max_start,
            HistoryScroll::Manual { top_line } => top_line.min(max_start),
        }
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        self.scroll = HistoryScroll::Bottom;
        self.hide();
    }

    pub(super) fn scroll_by(&mut self, content_len: usize, viewport_len: usize, delta: isize) {
        let max_start = content_len.saturating_sub(viewport_len);
        let next = self
            .visible_start(content_len, viewport_len)
            .saturating_add_signed(delta)
            .min(max_start);
        self.set_top_line(content_len, viewport_len, next);
    }

    pub(super) fn set_top_line(
        &mut self,
        content_len: usize,
        viewport_len: usize,
        top_line: usize,
    ) {
        self.scroll = scroll_state_for_top_line(content_len, viewport_len, top_line);
        if matches!(self.scroll, HistoryScroll::Bottom) {
            self.hide();
        } else {
            self.drag = None;
        }
    }

    pub(super) fn clamp(&mut self, content_len: usize, viewport_len: usize) {
        if matches!(self.scroll, HistoryScroll::Bottom) {
            self.drag = None;
            return;
        }
        if let HistoryScroll::Manual { top_line } = self.scroll {
            self.scroll = scroll_state_for_top_line(content_len, viewport_len, top_line);
            if matches!(self.scroll, HistoryScroll::Bottom) {
                self.hide();
            }
        }
    }

    pub(super) fn update_hover(
        &mut self,
        scrollbar: Option<HistoryScrollbar>,
        column: u16,
        row: u16,
    ) {
        self.hovered = scrollbar.is_some_and(|scrollbar| scrollbar.contains(column, row));
    }

    pub(super) fn begin_scrollbar_drag(
        &mut self,
        scrollbar: HistoryScrollbar,
        row: u16,
        now: Instant,
        reveal_duration: Duration,
    ) {
        self.reveal(now, reveal_duration);
        let drag = scrollbar.begin_drag(row);
        self.drag = Some(drag);
        self.scroll = scrollbar.scroll_state_for_pointer(row, drag);
    }

    pub(super) fn drag_to(&mut self, scrollbar: HistoryScrollbar, row: u16) {
        if let Some(drag) = self.drag {
            self.scroll = scrollbar.scroll_state_for_pointer(row, drag);
        }
    }
}

/// Inputs for scrollbar-only mouse handling (attach view).
pub(super) struct ScrollbarMouseInput {
    pub(super) now: Instant,
    pub(super) reveal_duration: Duration,
    pub(super) scrollbar: Option<HistoryScrollbar>,
    pub(super) content_len: usize,
    pub(super) viewport_len: usize,
    pub(super) wheel_lines: usize,
}

impl HistoryScrollChrome {
    /// Scrollbar-only mouse handling used by the read-only attach view.
    pub(super) fn handle_scrollbar_mouse(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        input: ScrollbarMouseInput,
    ) {
        let ScrollbarMouseInput {
            now,
            reveal_duration,
            scrollbar,
            content_len,
            viewport_len,
            wheel_lines,
        } = input;
        match kind {
            MouseEventKind::ScrollUp => {
                self.scroll_by(content_len, viewport_len, -(wheel_lines as isize));
                if !matches!(self.scroll, HistoryScroll::Bottom) {
                    self.reveal(now, reveal_duration);
                }
            }
            MouseEventKind::ScrollDown => {
                self.scroll_by(content_len, viewport_len, wheel_lines as isize);
                if !matches!(self.scroll, HistoryScroll::Bottom) {
                    self.reveal(now, reveal_duration);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = scrollbar
                    .filter(|scrollbar| scrollbar.contains(column, row))
                    .filter(|_| self.should_render(now));
                self.update_hover(scrollbar, column, row);
                if let Some(scrollbar) = hit {
                    self.begin_scrollbar_drag(scrollbar, row, now, reveal_duration);
                } else {
                    self.drag = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.update_hover(scrollbar, column, row);
                if let Some(scrollbar) = scrollbar {
                    self.drag_to(scrollbar, row);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
                self.update_hover(scrollbar, column, row);
            }
            MouseEventKind::Moved => {
                self.update_hover(scrollbar, column, row);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HistoryScrollbar {
    pub(super) rect: Rect,
    content_len: usize,
    viewport_len: usize,
    top_line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HistoryScrollbarDrag {
    Thumb {
        thumb_grab_offset: usize,
        start_row: usize,
        start_top_line: usize,
    },
    Track {
        thumb_grab_offset: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Thumb {
    top: usize,
    height: usize,
}

impl HistoryScrollbar {
    pub(super) fn new(history: Rect, content_len: usize, top_line: usize) -> Option<Self> {
        let viewport_len = history.height as usize;
        if history.width == 0 || !should_show(content_len, viewport_len) {
            return None;
        }

        Some(Self {
            rect: Rect::new(
                history.x.saturating_add(history.width.saturating_sub(1)),
                history.y,
                1,
                history.height,
            ),
            content_len,
            viewport_len,
            top_line,
        })
    }

    pub(super) fn contains(&self, column: u16, row: u16) -> bool {
        self.rect.contains((column, row).into())
    }

    pub(super) fn begin_drag(&self, row: u16) -> HistoryScrollbarDrag {
        let row = self.clamped_track_row(row);
        let thumb = self.thumb();
        if (thumb.top..thumb.top + thumb.height).contains(&row) {
            HistoryScrollbarDrag::Thumb {
                thumb_grab_offset: row.saturating_sub(thumb.top),
                start_row: row,
                start_top_line: self.top_line.min(self.max_top_line()),
            }
        } else {
            HistoryScrollbarDrag::Track {
                thumb_grab_offset: thumb.height / 2,
            }
        }
    }

    pub(super) fn top_line_for_pointer(&self, row: u16, drag: HistoryScrollbarDrag) -> usize {
        let row = self.clamped_track_row(row);
        let thumb_grab_offset = match drag {
            HistoryScrollbarDrag::Thumb {
                thumb_grab_offset,
                start_row,
                start_top_line,
            } => {
                if row == start_row {
                    return start_top_line;
                }
                thumb_grab_offset
            }
            HistoryScrollbarDrag::Track { thumb_grab_offset } => thumb_grab_offset,
        };
        let thumb = self.thumb();
        let max_thumb_top = (self.rect.height as usize).saturating_sub(thumb.height);
        if max_thumb_top == 0 {
            return 0;
        }
        let thumb_top = row.saturating_sub(thumb_grab_offset).min(max_thumb_top);
        rounding_divide(thumb_top.saturating_mul(self.max_top_line()), max_thumb_top)
    }

    pub(super) fn scroll_state_for_pointer(
        &self,
        row: u16,
        drag: HistoryScrollbarDrag,
    ) -> HistoryScroll {
        scroll_state_for_top_line(
            self.content_len,
            self.viewport_len,
            self.top_line_for_pointer(row, drag),
        )
    }

    pub(super) fn render(&self, frame: &mut Frame<'_>, dragging: bool) {
        let thumb = self.thumb();
        let track_style = Theme::dim().add_modifier(Modifier::DIM);
        let thumb_style = if dragging {
            Theme::brand()
        } else {
            Theme::accent()
        };
        let buffer = frame.buffer_mut();

        for row in 0..self.rect.height {
            let row_index = row as usize;
            let is_thumb = (thumb.top..thumb.top + thumb.height).contains(&row_index);
            let symbol = if is_thumb { "█" } else { "│" };
            let style = if is_thumb { thumb_style } else { track_style };
            buffer[(self.rect.x, self.rect.y.saturating_add(row))]
                .set_symbol(symbol)
                .set_style(style);
        }
    }

    fn thumb(&self) -> Thumb {
        let track_height = self.rect.height as usize;
        let height = rounding_divide(
            self.viewport_len.saturating_mul(track_height),
            self.content_len,
        )
        .clamp(1, track_height);
        let max_thumb_top = track_height.saturating_sub(height);
        let top = if max_thumb_top == 0 {
            0
        } else {
            rounding_divide(
                self.top_line
                    .min(self.max_top_line())
                    .saturating_mul(max_thumb_top),
                self.max_top_line(),
            )
            .min(max_thumb_top)
        };
        Thumb { top, height }
    }

    fn clamped_track_row(&self, row: u16) -> usize {
        if row <= self.rect.y {
            0
        } else {
            row.saturating_sub(self.rect.y)
                .min(self.rect.height.saturating_sub(1)) as usize
        }
    }

    fn max_top_line(&self) -> usize {
        self.content_len.saturating_sub(self.viewport_len)
    }
}

pub(super) fn scroll_state_for_top_line(
    content_len: usize,
    viewport_len: usize,
    top_line: usize,
) -> HistoryScroll {
    let max_top_line = content_len.saturating_sub(viewport_len);
    let top_line = top_line.min(max_top_line);
    if top_line >= max_top_line {
        HistoryScroll::Bottom
    } else {
        HistoryScroll::Manual { top_line }
    }
}

fn should_show(content_len: usize, viewport_len: usize) -> bool {
    viewport_len > 1 && content_len > viewport_len
}

fn rounding_divide(numerator: usize, denominator: usize) -> usize {
    (numerator + denominator / 2)
        .checked_div(denominator)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "scrollbar_tests.rs"]
mod tests;
