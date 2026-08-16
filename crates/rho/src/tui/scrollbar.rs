use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    Frame,
};

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

    /// Pin a top line for document-style reading.
    ///
    /// Unlike [`Self::set_top_line`], a top-of-document position stays top-anchored
    /// even when the content currently fits in the viewport. That keeps resize from
    /// flipping a short finished answer to bottom-stickiness.
    ///
    /// Does not clear drag/hover chrome; callers that change position from the
    /// keyboard should clear drag themselves if needed.
    pub(super) fn pin_top_line(
        &mut self,
        content_len: usize,
        viewport_len: usize,
        top_line: usize,
    ) {
        let max_start = content_len.saturating_sub(viewport_len);
        let top_line = top_line.min(max_start);
        self.scroll = if top_line == 0 {
            HistoryScroll::Manual { top_line: 0 }
        } else if top_line >= max_start {
            HistoryScroll::Bottom
        } else {
            HistoryScroll::Manual { top_line }
        };
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

/// Thumb geometry for a one-column scrollbar track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScrollbarThumb {
    top: usize,
    height: usize,
}

impl ScrollbarThumb {
    pub(super) fn contains(self, row: usize) -> bool {
        (self.top..self.top + self.height).contains(&row)
    }
}

const THUMB_GLYPH: &str = "█";
const TRACK_GLYPH: &str = "│";

/// Glyph and style for one row of a one-column scrollbar track.
///
/// The thumb style stays a caller argument because the history bar brightens
/// its thumb while dragging; the track and the glyphs are shared so every
/// scrollbar in the UI looks the same by construction.
pub(super) fn track_cell(
    thumb: ScrollbarThumb,
    row: usize,
    thumb_style: Style,
) -> (&'static str, Style) {
    if thumb.contains(row) {
        (THUMB_GLYPH, thumb_style)
    } else {
        (TRACK_GLYPH, Theme::dim().add_modifier(Modifier::DIM))
    }
}

/// [`track_cell`] as a span, for scrollbars rendered as line content.
pub(super) fn track_span(thumb: ScrollbarThumb, row: usize, thumb_style: Style) -> Span<'static> {
    let (glyph, style) = track_cell(thumb, row, thumb_style);
    Span::styled(glyph, style)
}

/// Thumb geometry for a `track_height` row scrollbar, or `None` when the
/// content fits the viewport and no bar should render.
pub(super) fn scrollbar_thumb(
    content_len: usize,
    viewport_len: usize,
    top_line: usize,
    track_height: usize,
) -> Option<ScrollbarThumb> {
    if track_height == 0 || !should_show(content_len, viewport_len) {
        return None;
    }
    let height = rounding_divide(viewport_len.saturating_mul(track_height), content_len)
        .clamp(1, track_height);
    let max_thumb_top = track_height.saturating_sub(height);
    let max_top_line = content_len.saturating_sub(viewport_len);
    let top = if max_thumb_top == 0 || max_top_line == 0 {
        0
    } else {
        rounding_divide(
            top_line.min(max_top_line).saturating_mul(max_thumb_top),
            max_top_line,
        )
        .min(max_thumb_top)
    };
    Some(ScrollbarThumb { top, height })
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
        if thumb.contains(row) {
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
        let thumb_style = if dragging {
            Theme::brand()
        } else {
            Theme::accent()
        };
        let buffer = frame.buffer_mut();

        for row in 0..self.rect.height {
            let (symbol, style) = track_cell(thumb, row as usize, thumb_style);
            buffer[(self.rect.x, self.rect.y.saturating_add(row))]
                .set_symbol(symbol)
                .set_style(style);
        }
    }

    fn thumb(&self) -> ScrollbarThumb {
        let track_height = self.rect.height as usize;
        // Construction already guarantees overflow; a full-track thumb is the
        // safe fallback if that invariant ever breaks.
        scrollbar_thumb(
            self.content_len,
            self.viewport_len,
            self.top_line,
            track_height,
        )
        .unwrap_or(ScrollbarThumb {
            top: 0,
            height: track_height.max(1),
        })
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

/// Lowest top line that may show the session header.
///
/// The header is intro chrome. Once the transcript body is taller than the
/// pane, scroll cannot move into those hint rows.
pub(super) fn history_scroll_min_start(
    header_len: usize,
    history_len: usize,
    viewport_len: usize,
) -> usize {
    let body_len = history_len.saturating_sub(header_len);
    if body_len >= viewport_len {
        header_len.min(history_len.saturating_sub(viewport_len))
    } else {
        0
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
