//! Pointer mechanics shared by single-pane overlays ([`OverlayPanelFrame`]).
//!
//! Every panel and the side chat own one [`PanelPointer`]. It turns pointer
//! events into scroll or copy effects against the frame the overlay painted:
//! wheel scrolling, scrollbar drag, drag-to-select with copy on release
//! ([`DragSelection`]), copy-target clicks, and hover feedback. Which overlay
//! is open and how it applies a scroll stay with the caller.

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::{
    layout::Position,
    widgets::{Clear, Paragraph},
    Frame,
};

use super::{
    drag_selection::DragSelection, overlay_panel::OverlayPanelFrame,
    scrollbar::HistoryScrollbarDrag, Theme, HISTORY_MOUSE_SCROLL_LINES,
};

/// Pointer state for one open overlay. Hover is not stored: paint resolves it
/// from the app's last pointer cell against the frame being painted, so a
/// move needs no handler work and a scroll or reflow under a still pointer
/// cannot leave stale hover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PanelPointer {
    selection: DragSelection,
    scrollbar_drag: Option<HistoryScrollbarDrag>,
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

/// The pointer events a panel acts on. Everything else, notably motion, is
/// resolved at paint time, so owners skip building a frame for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PanelPointerEvent {
    Wheel(isize),
    Press,
    Drag,
    Release,
}

impl PanelPointerEvent {
    /// The panel event for `kind`, or `None` when a panel ignores it.
    pub(super) fn from_kind(kind: MouseEventKind) -> Option<Self> {
        let wheel = HISTORY_MOUSE_SCROLL_LINES as isize;
        match kind {
            MouseEventKind::ScrollUp => Some(Self::Wheel(-wheel)),
            MouseEventKind::ScrollDown => Some(Self::Wheel(wheel)),
            MouseEventKind::Down(MouseButton::Left) => Some(Self::Press),
            MouseEventKind::Drag(MouseButton::Left) => Some(Self::Drag),
            MouseEventKind::Up(MouseButton::Left) => Some(Self::Release),
            MouseEventKind::Down(MouseButton::Right | MouseButton::Middle)
            | MouseEventKind::Drag(MouseButton::Right | MouseButton::Middle)
            | MouseEventKind::Up(MouseButton::Right | MouseButton::Middle)
            | MouseEventKind::Moved
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => None,
        }
    }
}

impl PanelPointer {
    /// Drops the selection, e.g. after the body text changed underneath it.
    pub(super) fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// Resolves one pointer event against `frame`, the overlay as painted.
    ///
    /// A press on a copy target copies instead of starting a selection; a
    /// press on the scrollbar starts a drag; any other press in the body
    /// anchors a selection that is copied on release if the pointer moved.
    pub(super) fn handle(
        &mut self,
        event: PanelPointerEvent,
        column: u16,
        row: u16,
        frame: &OverlayPanelFrame,
    ) -> PanelPointerEffect {
        match event {
            PanelPointerEvent::Wheel(delta) => {
                self.scrollbar_drag = None;
                PanelPointerEffect::ScrollBy(delta)
            }
            PanelPointerEvent::Press => self.press(column, row, frame),
            PanelPointerEvent::Drag => self.drag(column, row, frame),
            PanelPointerEvent::Release => self.release(column, row, frame),
        }
    }

    fn press(&mut self, column: u16, row: u16, frame: &OverlayPanelFrame) -> PanelPointerEffect {
        self.scrollbar_drag = None;
        self.selection.clear();
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
        self.selection.press(frame.selection_body(), column, row);
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
        self.selection.drag(frame.selection_body(), column, row);
        PanelPointerEffect::None
    }

    fn release(&mut self, column: u16, row: u16, frame: &OverlayPanelFrame) -> PanelPointerEffect {
        if self.scrollbar_drag.take().is_some() {
            return PanelPointerEffect::None;
        }
        self.selection
            .release(frame.selection_body(), column, row)
            .map_or(PanelPointerEffect::None, PanelPointerEffect::Copy)
    }

    /// Paints `overlay` with this pointer's feedback on top and returns the
    /// overlay caret. `at` is the app's last pointer cell, for hover.
    pub(super) fn paint_overlay(
        self,
        frame: &mut Frame<'_>,
        mut overlay: OverlayPanelFrame,
        at: Option<Position>,
    ) -> Option<Position> {
        // Clear punches host defaults; repaint the surface so light schemes
        // do not leave holes under the panel ink.
        frame.render_widget(Clear, overlay.outer);
        frame.render_widget(
            Paragraph::new(std::mem::take(&mut overlay.lines)).style(Theme::surface()),
            overlay.outer,
        );
        self.paint_feedback(frame, &overlay, at);
        overlay.cursor
    }

    /// The selection highlight, the hovered copy target, and the scrollbar
    /// thumb while hovered or dragged.
    fn paint_feedback(
        self,
        frame: &mut Frame<'_>,
        overlay: &OverlayPanelFrame,
        at: Option<Position>,
    ) {
        let body = overlay.body();
        self.selection
            .highlight(frame.buffer_mut(), body, overlay.scroll());
        let Some(at) = at else {
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
