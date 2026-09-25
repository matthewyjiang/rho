//! Drag-to-select over a scrolled body of lines.
//!
//! One mechanic for every scrolled text body that copies on drag: a left
//! press over the body anchors a selection, a drag extends it (clamped to the
//! body, so dragging past an edge selects up to that edge), and the release
//! yields the selected text. Positions are body lines, not screen rows, so the
//! highlight stays on its text while the body scrolls. Owners decide which
//! presses belong to other controls (scrollbars, copy targets) and what to do
//! with the copied text.

use ratatui::{buffer::Buffer, layout::Rect, text::Line};

use super::{
    copy_interaction::{selection_position, selection_position_clamped},
    text_selection::{highlight_selection, SelectionPosition, TextSelection},
};

/// A body as painted: its screen area, the body line in its top row, and
/// every body line (line 0 is the first body line, not the first visible one).
#[derive(Clone, Copy, Debug)]
pub(super) struct SelectionBody<'a> {
    pub(super) area: Rect,
    pub(super) top_line: usize,
    pub(super) lines: &'a [Line<'a>],
}

impl SelectionBody<'_> {
    fn clamped_position(self, column: u16, row: u16) -> Option<SelectionPosition> {
        selection_position_clamped(self.area, self.top_line, column, row)
    }
}

/// Selection state for one body. Owners forward left-button press, drag, and
/// release, and paint [`DragSelection::highlight`] over the body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct DragSelection {
    selection: Option<TextSelection>,
    /// A press anchored in the body is still held, so drags extend it.
    dragging: bool,
}

impl DragSelection {
    /// A selection is anchored or highlighted.
    pub(super) fn is_active(self) -> bool {
        self.selection.is_some()
    }

    /// Drops the selection, e.g. when the body text changed underneath it or
    /// another control took the press.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Anchors a selection when the press lands on body text; any other press
    /// drops the previous highlight.
    pub(super) fn press(&mut self, body: SelectionBody<'_>, column: u16, row: u16) {
        self.selection = selection_position(body.area, body.top_line, column, row)
            .filter(|_| !body.lines.is_empty())
            .map(TextSelection::new);
        self.dragging = self.selection.is_some();
    }

    pub(super) fn drag(&mut self, body: SelectionBody<'_>, column: u16, row: u16) {
        if self.dragging {
            self.extend(body, column, row);
        }
    }

    /// Ends the drag and returns the text it selected. A click without
    /// movement selects nothing and drops the anchor; a selection that copied
    /// text stays highlighted until the next press.
    pub(super) fn release(
        &mut self,
        body: SelectionBody<'_>,
        column: u16,
        row: u16,
    ) -> Option<String> {
        if !std::mem::take(&mut self.dragging) {
            return None;
        }
        self.extend(body, column, row);
        let text = self
            .selection
            .and_then(|selection| selection.selected_text(body.lines, /*first_line*/ 0));
        if text.is_none() {
            self.selection = None;
        }
        text
    }

    /// Reverses the selected cells of `area`, whose top row shows `top_line`.
    pub(super) fn highlight(self, buffer: &mut Buffer, area: Rect, top_line: usize) {
        if let Some(selection) = self.selection {
            highlight_selection(buffer, area, top_line, selection);
        }
    }

    fn extend(&mut self, body: SelectionBody<'_>, column: u16, row: u16) {
        if let (Some(selection), Some(position)) =
            (self.selection.as_mut(), body.clamped_position(column, row))
        {
            selection.update(position);
        }
    }
}

#[cfg(test)]
#[path = "drag_selection_tests.rs"]
mod tests;
