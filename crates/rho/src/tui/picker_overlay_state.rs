use super::UiPicker;

/// Which overlay pane keyboard scrolling acts on.
///
/// Only meaningful while an overlay picker shows a detail pane; nav-only
/// overlays and list pickers always scroll the nav list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) enum OverlayFocus {
    #[default]
    Nav,
    Detail,
}

/// Active overlay scrollbar drag (nav or detail track).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum OverlayScrollbarDrag {
    Nav(super::super::scrollbar::HistoryScrollbarDrag),
    Detail(super::super::scrollbar::HistoryScrollbarDrag),
}

impl UiPicker {
    pub(in crate::tui) fn has_scrollable_detail(&self) -> bool {
        self.is_overlay() && self.has_item_details()
    }

    pub(in crate::tui) fn focus_overlay_pane(&mut self, focus: OverlayFocus) {
        self.overlay_focus = focus;
    }

    /// Whether keyboard scrolling currently targets the detail pane.
    pub(in crate::tui) fn detail_pane_focused(&self) -> bool {
        self.has_scrollable_detail() && self.overlay_focus == OverlayFocus::Detail
    }

    /// First visible nav row for a `viewport_rows` tall nav pane.
    ///
    /// In keyboard mode the window moves the least amount that keeps the
    /// selection visible; after a wheel scroll it holds the manual offset even
    /// when the selection leaves the window.
    pub(in crate::tui) fn nav_window_start(&self, viewport_rows: usize) -> usize {
        let matching = self.matching_indices();
        let total = super::super::picker_rows::picker_row_count(&self.items, &matching);
        let viewport_rows = viewport_rows.max(1);
        let max_start = total.saturating_sub(viewport_rows);
        let base = self.nav_scroll.min(max_start);
        if !self.nav_follows_selection {
            return base;
        }
        let selected_row =
            super::super::picker_rows::selected_row_index(&self.items, &matching, self.selected);
        let lowest = super::super::picker_rows::scroll_window_start(selected_row, viewport_rows);
        let highest = selected_row.min(max_start);
        base.clamp(lowest.min(highest), highest)
    }

    /// Wheel scroll of the nav viewport without moving the selection.
    pub(in crate::tui) fn scroll_nav_by(&mut self, delta: isize, viewport_rows: usize) {
        let current = self.nav_window_start(viewport_rows);
        let max_start = {
            let matching = self.matching_indices();
            super::super::picker_rows::picker_row_count(&self.items, &matching)
                .saturating_sub(viewport_rows.max(1))
        };
        self.nav_scroll = current.saturating_add_signed(delta).min(max_start);
        self.nav_follows_selection = false;
    }

    /// Jump the nav viewport to an absolute top row without moving selection.
    pub(in crate::tui) fn scroll_nav_to(&mut self, top_line: usize, viewport_rows: usize) {
        let max_start = {
            let matching = self.matching_indices();
            super::super::picker_rows::picker_row_count(&self.items, &matching)
                .saturating_sub(viewport_rows.max(1))
        };
        self.nav_scroll = top_line.min(max_start);
        self.nav_follows_selection = false;
    }

    pub(in crate::tui) fn overlay_scrollbar_drag(&self) -> Option<OverlayScrollbarDrag> {
        self.overlay_scrollbar_drag
    }

    pub(in crate::tui) fn set_overlay_scrollbar_drag(
        &mut self,
        drag: Option<OverlayScrollbarDrag>,
    ) {
        self.overlay_scrollbar_drag = drag;
    }

    /// Nav row under the mouse pointer, in row space.
    pub(in crate::tui) fn hovered_nav_row(&self) -> Option<usize> {
        self.hovered_nav_row
    }

    /// Record the nav row under the mouse pointer, or `None` off the rows.
    pub(in crate::tui) fn set_hovered_nav_row(&mut self, row_index: Option<usize>) {
        self.hovered_nav_row = row_index;
    }

    /// Item index shown at a row-space nav row, skipping section headers.
    pub(in crate::tui) fn nav_item_at_row(&self, row_index: usize) -> Option<usize> {
        let matching = self.matching_indices();
        super::super::picker_rows::item_index_at_row(&self.items, &matching, row_index)
    }

    /// Select the item at a row-space nav row (mouse click).
    ///
    /// Pins the current window first so the click never shifts the viewport.
    pub(in crate::tui) fn select_nav_row(
        &mut self,
        row_index: usize,
        viewport_rows: usize,
    ) -> bool {
        let Some(index) = self.nav_item_at_row(row_index) else {
            return false;
        };
        self.nav_scroll = self.nav_window_start(viewport_rows);
        self.nav_follows_selection = true;
        if index != self.selected {
            self.selected = index;
            self.on_selection_changed();
        }
        true
    }

    /// Content hints the overlay uses to size its outer box.
    pub(in crate::tui) fn overlay_sizing(
        &self,
    ) -> super::super::picker_overlay_layout::OverlaySizing {
        super::super::picker_overlay_layout::OverlaySizing {
            has_details: self.has_item_details(),
            nav_rows: super::super::picker_rows::rows(&self.items, 0..self.items.len()).count(),
        }
    }
}
