//! Picker overlay geometry: outer sizing, pane split, scroll targets, hit testing.
//!
//! This module owns every measurement the overlay renderer and the overlay
//! input router both depend on, so a chrome change cannot leave the two out of
//! step. Rendering lives in [`super::picker_overlay`].

use ratatui::layout::{Position, Rect};

use super::{
    display_width,
    scrollbar::{scrollbar_thumb, HistoryScrollbar, ScrollbarThumb},
};

const TWO_COLUMN_MIN_INNER_WIDTH: usize = 60;
const MIN_NAV_WIDTH: usize = 14;
const MAX_NAV_WIDTH: usize = 28;
/// Column rule drawn between the side-by-side panes.
pub(super) const SEPARATOR: &str = " │ ";
/// Border rows the renderer draws above and below the inner chrome.
pub(super) const TOP_BORDER_ROWS: usize = 1;
pub(super) const BOTTOM_BORDER_ROWS: usize = 1;
/// Rows inside the border above the body: search, divider, pane header.
pub(super) const HEADER_CHROME_ROWS: usize = 3;
/// Rows inside the border below the body: status divider, footer.
pub(super) const FOOTER_CHROME_ROWS: usize = 2;
/// Rows consumed inside the border by chrome, above and below the body.
const INNER_CHROME_ROWS: usize = HEADER_CHROME_ROWS + FOOTER_CHROME_ROWS;
/// Column reserved beside the detail text for its scrollbar, so wrapped text
/// never re-flows when the bar appears.
const DETAIL_SCROLLBAR_GUTTER: usize = 1;
/// Narrowest nav pane that still spends a column on a scrollbar.
const MIN_SCROLLBAR_PANE_WIDTH: usize = 4;
/// Fewest body rows an overlay keeps when a detail pane needs reading room.
const MIN_DETAIL_BODY_ROWS: usize = 12;
/// Fewest body rows a nav-only overlay keeps.
const MIN_NAV_ONLY_BODY_ROWS: usize = 3;

/// Responsive arrangement of nav + detail. Only used when a detail pane exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OverlayOrientation {
    SideBySide,
    Stacked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OverlayPanes {
    NavOnly {
        nav_width: usize,
        nav_viewport_rows: usize,
    },
    NavAndDetail {
        orientation: OverlayOrientation,
        nav_width: usize,
        detail_width: usize,
        detail_viewport_rows: usize,
        nav_viewport_rows: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OverlayLayout {
    pub(super) outer: Rect,
    pub(super) inner_width: usize,
    pub(super) inner_height: usize,
    pub(super) body_rows: usize,
    pub(super) panes: OverlayPanes,
}

/// Canonical visibility and position of one picker overlay scrollbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OverlayScrollbarState {
    content_len: usize,
    viewport_rows: usize,
    top_line: usize,
}

impl OverlayScrollbarState {
    pub(super) fn nav(
        pane_width: usize,
        content_len: usize,
        viewport_rows: usize,
        top_line: usize,
    ) -> Option<Self> {
        if pane_width <= MIN_SCROLLBAR_PANE_WIDTH {
            return None;
        }
        Self::visible(content_len, viewport_rows, top_line)
    }

    pub(super) fn detail(
        content_len: usize,
        viewport_rows: usize,
        top_line: usize,
    ) -> Option<Self> {
        Self::visible(content_len, viewport_rows, top_line)
    }

    fn visible(content_len: usize, viewport_rows: usize, top_line: usize) -> Option<Self> {
        let top_line = clamp_overlay_scroll(top_line, content_len, viewport_rows);
        scrollbar_thumb(content_len, viewport_rows, top_line, viewport_rows).map(|_| Self {
            content_len,
            viewport_rows,
            top_line,
        })
    }

    pub(super) fn thumb(self) -> ScrollbarThumb {
        scrollbar_thumb(
            self.content_len,
            self.viewport_rows,
            self.top_line,
            self.viewport_rows,
        )
        .expect("visible scrollbar state has a thumb")
    }

    pub(super) fn hitbox(self, rect: Rect) -> HistoryScrollbar {
        HistoryScrollbar::new(rect, self.content_len, self.top_line)
            .expect("visible scrollbar state has a hitbox")
    }
}

pub(super) fn clamp_overlay_scroll(
    top_line: usize,
    content_len: usize,
    viewport_rows: usize,
) -> usize {
    top_line.min(content_len.saturating_sub(viewport_rows.max(1)))
}

impl OverlayLayout {
    pub(super) fn detail_viewport(self) -> Option<DetailViewport> {
        match self.panes {
            OverlayPanes::NavOnly { .. } => None,
            OverlayPanes::NavAndDetail {
                detail_width,
                detail_viewport_rows,
                ..
            } => Some(DetailViewport {
                width: detail_width,
                rows: detail_viewport_rows,
            }),
        }
    }

    pub(super) fn scroll_targets(self) -> OverlayScrollTargets {
        OverlayScrollTargets {
            nav_rows: self.nav_viewport_rows().max(1),
            detail: self.detail_viewport(),
        }
    }

    /// Screen row of the first body row, derived from the same chrome counts the
    /// renderer builds its header rows from.
    pub(super) fn body_top(self) -> u16 {
        self.outer
            .y
            .saturating_add(as_u16(TOP_BORDER_ROWS.saturating_add(HEADER_CHROME_ROWS)))
    }

    /// Column of the side-by-side pane rule, measured from the overlay's left
    /// border. Shared by the border rules and by hit testing so the drawn rule
    /// and the pane boundary can never drift apart.
    pub(super) fn divider_col(self) -> Option<usize> {
        match self.panes {
            OverlayPanes::NavAndDetail {
                orientation: OverlayOrientation::SideBySide,
                nav_width,
                ..
            } => {
                // left border │ + nav + leading space of " │ "
                Some(1usize.saturating_add(nav_width).saturating_add(1))
            }
            OverlayPanes::NavAndDetail {
                orientation: OverlayOrientation::Stacked,
                ..
            }
            | OverlayPanes::NavOnly { .. } => None,
        }
    }

    /// Overlay pane and pane-local row under a screen position, for wheel,
    /// click, and hover routing. `None` when the position sits outside the
    /// body rows (chrome, or off the overlay).
    pub(super) fn pane_hit(self, column: u16, row: u16) -> Option<PaneHit> {
        let outer = self.outer;
        if !outer.contains(Position { x: column, y: row }) {
            return None;
        }
        let body_top = self.body_top();
        let body_bottom = body_top.saturating_add(as_u16(self.body_rows));
        if row < body_top || row >= body_bottom {
            return None;
        }
        let body_row = row.saturating_sub(body_top) as usize;
        match self.panes {
            OverlayPanes::NavOnly { .. } => Some(PaneHit {
                pane: OverlayPane::Nav,
                pane_row: body_row,
            }),
            OverlayPanes::NavAndDetail {
                orientation: OverlayOrientation::SideBySide,
                ..
            } => {
                // Columns left of the drawn pane rule belong to nav.
                let nav_end = outer
                    .x
                    .saturating_add(as_u16(self.divider_col().unwrap_or_default()));
                let pane = if column < nav_end {
                    OverlayPane::Nav
                } else {
                    OverlayPane::Detail
                };
                Some(PaneHit {
                    pane,
                    pane_row: body_row,
                })
            }
            OverlayPanes::NavAndDetail {
                orientation: OverlayOrientation::Stacked,
                detail_viewport_rows,
                ..
            } => {
                if body_row < detail_viewport_rows {
                    return Some(PaneHit {
                        pane: OverlayPane::Detail,
                        pane_row: body_row,
                    });
                }
                // The rule row between the stacked panes is chrome.
                let nav_top = detail_viewport_rows.saturating_add(self.stacked_separator_rows());
                if body_row < nav_top {
                    return None;
                }
                Some(PaneHit {
                    pane: OverlayPane::Nav,
                    pane_row: body_row - nav_top,
                })
            }
        }
    }

    /// One body row is the horizontal rule between stacked detail and nav when
    /// there is room for both panes plus the separator.
    fn stacked_separator_rows(self) -> usize {
        usize::from(self.body_rows > 2)
    }

    pub(super) fn nav_width(self) -> usize {
        match self.panes {
            OverlayPanes::NavOnly { nav_width, .. }
            | OverlayPanes::NavAndDetail { nav_width, .. } => nav_width,
        }
    }

    pub(super) fn nav_viewport_rows(self) -> usize {
        match self.panes {
            OverlayPanes::NavOnly {
                nav_viewport_rows, ..
            }
            | OverlayPanes::NavAndDetail {
                nav_viewport_rows, ..
            } => nav_viewport_rows,
        }
    }

    /// Screen rectangle covering the nav item rows (inside the border).
    pub(super) fn nav_body_rect(self) -> Rect {
        let x = self.outer.x.saturating_add(1);
        let width = as_u16(self.nav_width().max(1));
        let height = as_u16(self.nav_viewport_rows().max(1));
        let y = match self.panes {
            OverlayPanes::NavOnly { .. }
            | OverlayPanes::NavAndDetail {
                orientation: OverlayOrientation::SideBySide,
                ..
            } => self.body_top(),
            OverlayPanes::NavAndDetail {
                orientation: OverlayOrientation::Stacked,
                detail_viewport_rows,
                ..
            } => self.body_top().saturating_add(as_u16(
                detail_viewport_rows.saturating_add(self.stacked_separator_rows()),
            )),
        };
        Rect::new(x, y, width, height)
    }

    /// Screen rectangle covering the detail pane rows (inside the border).
    pub(super) fn detail_body_rect(self) -> Option<Rect> {
        let viewport = self.detail_viewport()?;
        let x = match self.panes {
            OverlayPanes::NavOnly { .. } => return None,
            OverlayPanes::NavAndDetail {
                orientation: OverlayOrientation::SideBySide,
                ..
            } => {
                // Content after left border + nav + separator.
                let offset = 1usize
                    .saturating_add(self.nav_width())
                    .saturating_add(display_width(SEPARATOR));
                self.outer.x.saturating_add(as_u16(offset))
            }
            OverlayPanes::NavAndDetail {
                orientation: OverlayOrientation::Stacked,
                ..
            } => self.outer.x.saturating_add(1),
        };
        // Detail text width plus its reserved scrollbar gutter.
        let width = as_u16(
            viewport
                .width
                .saturating_add(DETAIL_SCROLLBAR_GUTTER)
                .max(1),
        );
        Some(Rect::new(
            x,
            self.body_top(),
            width,
            as_u16(viewport.rows.max(1)),
        ))
    }
}

/// Scroll geometry for an open overlay: nav viewport rows plus the detail
/// viewport when a detail pane exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OverlayScrollTargets {
    pub(super) nav_rows: usize,
    pub(super) detail: Option<DetailViewport>,
}

/// One of the two overlay panes, for focus and wheel routing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OverlayPane {
    Nav,
    Detail,
}

/// A body position resolved to a pane plus the row within that pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PaneHit {
    pub(super) pane: OverlayPane,
    pub(super) pane_row: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DetailViewport {
    pub(super) width: usize,
    pub(super) rows: usize,
}

/// Content hints that drive the overlay's outer size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OverlaySizing {
    pub(super) has_details: bool,
    /// Item rows plus section headers, counted over the full item set (not the
    /// filtered matches) so the box does not resize while typing.
    pub(super) nav_rows: usize,
}

pub(super) fn picker_overlay_layout(area: Rect, sizing: OverlaySizing) -> OverlayLayout {
    layout_for_outer(outer_rect(area, sizing), sizing.has_details)
}

fn outer_rect(area: Rect, sizing: OverlaySizing) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::new(area.x, area.y, 0, 0);
    }

    let horizontal_margin = ((area.width as usize) / 20).clamp(1, 4) as u16;
    let vertical_margin = ((area.height as usize) / 12).clamp(1, 3) as u16;
    let width = area
        .width
        .saturating_sub(horizontal_margin.saturating_mul(2))
        .max(1);
    let max_height = area
        .height
        .saturating_sub(vertical_margin.saturating_mul(2))
        .max(1);
    // Height follows the item count instead of always filling the screen, so
    // short pickers render as a compact box. The detail minimum keeps long
    // detail text readable behind its own scrolling.
    let min_body_rows = if sizing.has_details {
        MIN_DETAIL_BODY_ROWS
    } else {
        MIN_NAV_ONLY_BODY_ROWS
    };
    let desired_height = as_u16(
        sizing
            .nav_rows
            .max(min_body_rows)
            .saturating_add(INNER_CHROME_ROWS)
            .saturating_add(TOP_BORDER_ROWS)
            .saturating_add(BOTTOM_BORDER_ROWS),
    );
    let height = desired_height.min(max_height);
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    Rect::new(x, y, width, height)
}

fn layout_for_outer(outer: Rect, has_details: bool) -> OverlayLayout {
    let outer_width = outer.width as usize;
    let outer_height = outer.height as usize;
    let inner_width = outer_width.saturating_sub(2).max(1);
    let inner_height = outer_height.saturating_sub(2).max(1);
    let body_rows = inner_height.saturating_sub(INNER_CHROME_ROWS).max(1);

    let panes = if !has_details {
        OverlayPanes::NavOnly {
            nav_width: inner_width,
            nav_viewport_rows: body_rows,
        }
    } else if inner_width < TWO_COLUMN_MIN_INNER_WIDTH {
        // One body row is the horizontal rule between detail and nav when there
        // is room for both panes plus the separator.
        let separator_rows = usize::from(body_rows > 2);
        let usable_rows = body_rows.saturating_sub(separator_rows).max(1);
        let detail_viewport_rows = (usable_rows.saturating_mul(3) / 5)
            .max(2.min(usable_rows.saturating_sub(1)))
            .min(usable_rows.saturating_sub(1));
        let nav_viewport_rows = usable_rows.saturating_sub(detail_viewport_rows);
        OverlayPanes::NavAndDetail {
            orientation: OverlayOrientation::Stacked,
            nav_width: inner_width,
            // One column stays reserved for the detail scrollbar gutter so the
            // wrapped text never re-flows when the bar appears.
            detail_width: inner_width.saturating_sub(DETAIL_SCROLLBAR_GUTTER).max(1),
            detail_viewport_rows,
            nav_viewport_rows,
        }
    } else {
        let nav_width = ((inner_width * 30) / 100).clamp(MIN_NAV_WIDTH, MAX_NAV_WIDTH);
        let separator_width = display_width(SEPARATOR);
        let detail_width = inner_width
            .saturating_sub(nav_width)
            .saturating_sub(separator_width)
            .saturating_sub(DETAIL_SCROLLBAR_GUTTER)
            .max(1);
        OverlayPanes::NavAndDetail {
            orientation: OverlayOrientation::SideBySide,
            nav_width,
            detail_width,
            detail_viewport_rows: body_rows,
            nav_viewport_rows: body_rows,
        }
    };

    OverlayLayout {
        outer,
        inner_width,
        inner_height,
        body_rows,
        panes,
    }
}

fn as_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

#[cfg(test)]
#[path = "picker_overlay_layout_tests.rs"]
mod tests;
