//! Screen-space viewport and mouse state of the graph pane.

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::Rect;

use crate::tui::terminal_graph::NodeRect;

use super::dag::{to_u16, DagRender};

/// Rows moved per wheel step, matching the history view's wheel feel.
const WHEEL_PAN_ROWS: u16 = 3;

/// What one mouse event did to the graph pane.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DagMouse {
    /// The event does not belong to the graph pane.
    Ignored,
    /// The viewport moved or a drag started or ended; redraw the screen.
    Redraw,
    /// A click landed on this node.
    SelectNode(usize),
}

#[derive(Clone, Copy, Debug, Default)]
struct DagDrag {
    press_column: u16,
    press_row: u16,
    origin: (u16, u16),
    moved: bool,
}

/// Screen-space state of the graph pane: the geometry of the last draw plus a
/// user pan that overrides follow-the-selection until keyboard navigation.
#[derive(Debug, Default)]
pub(super) struct DagPane {
    inner: Rect,
    canvas: (usize, usize),
    node_rects: Vec<NodeRect>,
    drawn_offset: (u16, u16),
    manual_offset: Option<(u16, u16)>,
    drag: Option<DagDrag>,
}

impl DagPane {
    pub(super) fn node_rects(&self) -> &[NodeRect] {
        &self.node_rects
    }

    /// Return to following the selected node.
    pub(super) fn clear_manual_offset(&mut self) {
        self.manual_offset = None;
    }

    /// Resolve the `(row, column)` scroll for this draw and remember the
    /// geometry for later mouse hit tests.
    pub(super) fn offset_for_draw(
        &mut self,
        render: &DagRender,
        selected: usize,
        inner: Rect,
    ) -> (u16, u16) {
        // Adopt this draw's geometry first so the manual offset clamps against
        // the canvas it will be drawn on, not the previous one.
        self.inner = inner;
        self.canvas = (render.canvas_width, render.canvas_height);
        self.node_rects.clone_from(&render.node_rects);
        let offset = match self.manual_offset {
            Some(offset) => {
                let clamped = self.clamp_to_canvas(offset);
                self.manual_offset = Some(clamped);
                clamped
            }
            None => render.viewport_offset(selected, inner.width, inner.height),
        };
        self.drawn_offset = offset;
        offset
    }

    /// Drag pans the canvas, a press-and-release without movement selects the
    /// node under the pointer, and the wheel pans vertically.
    pub(super) fn handle_mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) -> DagMouse {
        let inside = self.inner.contains((column, row).into());
        match (kind, self.drag) {
            (MouseEventKind::Down(MouseButton::Left), _) if inside => {
                self.drag = Some(DagDrag {
                    press_column: column,
                    press_row: row,
                    origin: self.drawn_offset,
                    moved: false,
                });
                DagMouse::Redraw
            }
            (MouseEventKind::Drag(MouseButton::Left), Some(mut drag)) => {
                let panned = self.clamp_to_canvas((
                    pan_axis(drag.origin.0, drag.press_row, row),
                    pan_axis(drag.origin.1, drag.press_column, column),
                ));
                drag.moved |= panned != self.drawn_offset;
                self.drag = Some(drag);
                self.manual_offset = Some(panned);
                DagMouse::Redraw
            }
            (MouseEventKind::Up(MouseButton::Left), Some(drag)) => {
                self.drag = None;
                if drag.moved {
                    return DagMouse::Redraw;
                }
                match self.node_at(drag.press_column, drag.press_row) {
                    Some(index) => DagMouse::SelectNode(index),
                    None => DagMouse::Redraw,
                }
            }
            (MouseEventKind::ScrollUp, _) if inside => self.wheel_pan(-i32::from(WHEEL_PAN_ROWS)),
            (MouseEventKind::ScrollDown, _) if inside => self.wheel_pan(i32::from(WHEEL_PAN_ROWS)),
            _ => DagMouse::Ignored,
        }
    }

    fn wheel_pan(&mut self, delta_rows: i32) -> DagMouse {
        let current = self.manual_offset.unwrap_or(self.drawn_offset);
        let row = clamp_u16(i32::from(current.0) + delta_rows);
        let panned = self.clamp_to_canvas((row, current.1));
        self.manual_offset = Some(panned);
        DagMouse::Redraw
    }

    fn clamp_to_canvas(&self, offset: (u16, u16)) -> (u16, u16) {
        (
            offset.0.min(to_u16(
                self.canvas.1.saturating_sub(self.inner.height.into()),
            )),
            offset.1.min(to_u16(
                self.canvas.0.saturating_sub(self.inner.width.into()),
            )),
        )
    }

    /// Map a screen position through the drawn offset into canvas space.
    fn node_at(&self, column: u16, row: u16) -> Option<usize> {
        if !self.inner.contains((column, row).into()) {
            return None;
        }
        let x = usize::from(column - self.inner.x) + usize::from(self.drawn_offset.1);
        let y = usize::from(row - self.inner.y) + usize::from(self.drawn_offset.0);
        self.node_rects.iter().position(|rect| {
            x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
        })
    }
}

fn pan_axis(origin: u16, press: u16, current: u16) -> u16 {
    // Dragging moves the canvas with the pointer: content follows the mouse.
    clamp_u16(i32::from(origin) + i32::from(press) - i32::from(current))
}

fn clamp_u16(value: i32) -> u16 {
    value.clamp(0, i32::from(u16::MAX)) as u16
}

#[cfg(test)]
#[path = "dag_pane_tests.rs"]
mod tests;
