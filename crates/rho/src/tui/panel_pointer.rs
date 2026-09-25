//! Pointer mechanics shared by single-pane overlays ([`OverlayPanelFrame`]).
//!
//! Every panel and the side chat own one [`PanelPointer`]. It turns pointer
//! events into scroll or copy effects against the frame the overlay painted:
//! wheel scrolling, scrollbar drag, drag-to-select with copy on release,
//! copy-target clicks, and hover feedback. Which overlay is open and how it
//! applies a scroll stay with the caller.

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::{
    layout::Position,
    widgets::{Clear, Paragraph},
    Frame,
};

use super::{
    copy_interaction::{selection_position, selection_position_clamped},
    overlay_panel::OverlayPanelFrame,
    scrollbar::HistoryScrollbarDrag,
    text_selection::{highlight_selection, TextSelection},
    Theme, HISTORY_MOUSE_SCROLL_LINES,
};

/// Pointer state for one open overlay. Selection lines are body lines, not
/// screen rows, so a highlight stays on its text while the body scrolls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PanelPointer {
    selection: Option<TextSelection>,
    scrollbar_drag: Option<HistoryScrollbarDrag>,
    /// Last pointer cell. Hover is resolved against the frame being painted,
    /// so a scroll or reflow under a still pointer cannot leave stale hover.
    at: Option<Position>,
}

/// What the owning overlay must do after one pointer event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PanelPointerEffect {
    None,
    /// Scroll the body so this line is the top row.
    ScrollTo(usize),
    ScrollBy(isize),
    Copy(String),
}

impl PanelPointer {
    /// Drops the selection, e.g. after the body text changed underneath it.
    pub(super) fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Resolves one pointer event against `frame`, the overlay as painted.
    ///
    /// A press on a copy target copies instead of starting a selection; a
    /// press on the scrollbar starts a drag; any other press in the body
    /// anchors a selection that is copied on release if the pointer moved.
    pub(super) fn handle(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        frame: &OverlayPanelFrame,
    ) -> PanelPointerEffect {
        self.at = Some(Position { x: column, y: row });
        let wheel = HISTORY_MOUSE_SCROLL_LINES as isize;
        match kind {
            MouseEventKind::ScrollUp => {
                self.scrollbar_drag = None;
                PanelPointerEffect::ScrollBy(-wheel)
            }
            MouseEventKind::ScrollDown => {
                self.scrollbar_drag = None;
                PanelPointerEffect::ScrollBy(wheel)
            }
            MouseEventKind::Down(MouseButton::Left) => self.press(column, row, frame),
            MouseEventKind::Drag(MouseButton::Left) => self.drag(column, row, frame),
            MouseEventKind::Up(MouseButton::Left) => self.release(column, row, frame),
            _ => PanelPointerEffect::None,
        }
    }

    fn press(&mut self, column: u16, row: u16, frame: &OverlayPanelFrame) -> PanelPointerEffect {
        self.scrollbar_drag = None;
        self.selection = None;
        if let Some(hit) = frame.copy_hit_at(column, row) {
            return PanelPointerEffect::Copy(hit.text.clone());
        }
        if let Some(scrollbar) = frame
            .scrollbar()
            .filter(|scrollbar| scrollbar.contains(column, row))
        {
            let drag = scrollbar.begin_drag(row);
            self.scrollbar_drag = Some(drag);
            return PanelPointerEffect::ScrollTo(scrollbar.top_line_for_pointer(row, drag));
        }
        self.selection =
            selection_position(frame.body(), frame.scroll(), column, row).map(TextSelection::new);
        PanelPointerEffect::None
    }

    fn drag(&mut self, column: u16, row: u16, frame: &OverlayPanelFrame) -> PanelPointerEffect {
        if let Some(drag) = self.scrollbar_drag {
            return frame
                .scrollbar()
                .map_or(PanelPointerEffect::None, |scrollbar| {
                    PanelPointerEffect::ScrollTo(scrollbar.top_line_for_pointer(row, drag))
                });
        }
        if let (Some(selection), Some(position)) = (
            self.selection.as_mut(),
            selection_position_clamped(frame.body(), frame.scroll(), column, row),
        ) {
            selection.update(position);
        }
        PanelPointerEffect::None
    }

    fn release(&mut self, column: u16, row: u16, frame: &OverlayPanelFrame) -> PanelPointerEffect {
        if self.scrollbar_drag.take().is_some() {
            return PanelPointerEffect::None;
        }
        let Some(mut selection) = self.selection.take() else {
            return PanelPointerEffect::None;
        };
        if let Some(position) =
            selection_position_clamped(frame.body(), frame.scroll(), column, row)
        {
            selection.update(position);
        }
        // A click without movement selects nothing and drops the anchor.
        let Some(text) = selection.selected_text(frame.body_lines(), /*first_line*/ 0) else {
            return PanelPointerEffect::None;
        };
        // Keep the copied span highlighted until the next press.
        self.selection = Some(selection);
        PanelPointerEffect::Copy(text)
    }

    /// Paints `overlay` with this pointer's feedback on top and returns the
    /// overlay caret.
    pub(super) fn paint_overlay(
        self,
        frame: &mut Frame<'_>,
        mut overlay: OverlayPanelFrame,
    ) -> Option<Position> {
        // Clear punches host defaults; repaint the surface so light schemes
        // do not leave holes under the panel ink.
        frame.render_widget(Clear, overlay.outer);
        frame.render_widget(
            Paragraph::new(std::mem::take(&mut overlay.lines)).style(Theme::surface()),
            overlay.outer,
        );
        self.paint_feedback(frame, &overlay);
        overlay.cursor
    }

    /// The selection highlight, the hovered copy target, and the scrollbar
    /// thumb while hovered or dragged.
    fn paint_feedback(self, frame: &mut Frame<'_>, overlay: &OverlayPanelFrame) {
        let body = overlay.body();
        let scroll = overlay.scroll();
        if let Some(selection) = self.selection {
            highlight_selection(frame.buffer_mut(), body, scroll, selection);
        }
        let Some(at) = self.at else {
            return;
        };
        if let Some(hit) = overlay.copy_hit_at(at.x, at.y) {
            // The hit test places the target on the pointer's row, inside the body.
            let columns = hit.columns.clone();
            for column in columns.take_while(|&column| column < body.width as usize) {
                frame.buffer_mut()[(body.x.saturating_add(column as u16), at.y)]
                    .set_style(Theme::markdown_code_copy_button(/*hovered*/ true));
            }
        }
        // Hover lights the thumb the way the history bar lights it mid-drag.
        if let Some(scrollbar) = overlay
            .scrollbar()
            .filter(|scrollbar| self.scrollbar_drag.is_some() || scrollbar.contains(at.x, at.y))
        {
            scrollbar.render(frame, /*dragging*/ true);
        }
    }
}

#[cfg(test)]
#[path = "panel_pointer_tests.rs"]
mod tests;
