use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::*;

fn nav_and_detail_panes(layout: &OverlayLayout) -> OverlayPanes {
    match layout.panes {
        panes @ OverlayPanes::NavAndDetail { .. } => panes,
        OverlayPanes::NavOnly { .. } => panic!("expected nav+detail panes, got nav-only"),
    }
}

#[test]
fn tiny_stacked_layout_keeps_viewports_within_the_body() {
    let layout = picker_overlay_layout(
        Rect::new(0, 0, 20, 1),
        OverlaySizing {
            has_details: true,
            nav_rows: 2,
        },
    );
    let OverlayPanes::NavAndDetail {
        orientation,
        detail_viewport_rows,
        nav_viewport_rows,
        ..
    } = nav_and_detail_panes(&layout)
    else {
        unreachable!()
    };

    assert_eq!(orientation, OverlayOrientation::Stacked);
    let separator_rows = usize::from(layout.body_rows > 2);
    assert!(detail_viewport_rows + nav_viewport_rows + separator_rows <= layout.body_rows);
    assert_eq!(nav_viewport_rows, 1);
}

// Covers: the overlay height must follow the item count instead of always
// filling the screen; short pickers get a compact box, long pickers still
// clamp to the margin-bounded maximum.
// Owner: tui picker_overlay geometry
#[test]
fn overlay_height_follows_item_count() {
    let area = Rect::new(0, 0, 120, 40);
    let small = picker_overlay_layout(
        area,
        OverlaySizing {
            has_details: true,
            nav_rows: 5,
        },
    );
    // 12-row detail minimum + 5 chrome rows + 2 border rows.
    assert_eq!(small.outer.height, 19);

    let nav_only = picker_overlay_layout(
        area,
        OverlaySizing {
            has_details: false,
            nav_rows: 5,
        },
    );
    // 5 nav rows + 5 chrome rows + 2 border rows.
    assert_eq!(nav_only.outer.height, 12);

    let large = picker_overlay_layout(
        area,
        OverlaySizing {
            has_details: true,
            nav_rows: 100,
        },
    );
    // Clamped to the screen minus vertical margins.
    assert_eq!(large.outer.height, 34);
    assert!(large.outer.y > area.y);
}

// Covers: wheel routing must hit the pane under the pointer for both
// orientations and ignore chrome rows, or the wrong pane scrolls.
// Owner: tui picker_overlay geometry
#[test]
fn pane_at_maps_positions_to_panes() {
    let area = Rect::new(0, 0, 120, 40);
    let sizing = OverlaySizing {
        has_details: true,
        nav_rows: 30,
    };
    let side_by_side = picker_overlay_layout(area, sizing);
    let outer = side_by_side.outer;
    let OverlayPanes::NavAndDetail { nav_width, .. } = side_by_side.panes else {
        panic!("expected nav+detail");
    };
    let body_row = side_by_side.body_top();
    assert_eq!(
        side_by_side
            .pane_hit(outer.x + 2, body_row)
            .map(|hit| hit.pane),
        Some(OverlayPane::Nav)
    );
    assert_eq!(
        side_by_side
            .pane_hit(outer.x + 2 + nav_width as u16 + 3, body_row)
            .map(|hit| hit.pane),
        Some(OverlayPane::Detail)
    );
    // Chrome rows (title, filter) and space outside the overlay hit nothing.
    assert_eq!(side_by_side.pane_hit(outer.x + 2, outer.y + 1), None);
    assert_eq!(side_by_side.pane_hit(0, 0), None);

    // Narrow terminals stack detail above nav.
    let stacked = picker_overlay_layout(Rect::new(0, 0, 40, 40), sizing);
    let outer = stacked.outer;
    let OverlayPanes::NavAndDetail {
        detail_viewport_rows,
        ..
    } = stacked.panes
    else {
        panic!("expected nav+detail");
    };
    let body_top = stacked.body_top();
    assert_eq!(
        stacked.pane_hit(outer.x + 2, body_top).map(|hit| hit.pane),
        Some(OverlayPane::Detail)
    );
    // The rule row between the stacked panes is chrome, not a pane.
    assert_eq!(
        stacked.pane_hit(outer.x + 2, body_top + detail_viewport_rows as u16),
        None
    );
    assert_eq!(
        stacked.pane_hit(outer.x + 2, body_top + detail_viewport_rows as u16 + 1),
        Some(PaneHit {
            pane: OverlayPane::Nav,
            pane_row: 0
        })
    );
}

// Covers: nav scrollbar hit testing must use the same body rect the renderer
// paints into, or track clicks select rows instead of scrolling.
// Owner: pure unit (overlay geometry)
#[test]
fn nav_body_rect_right_edge_is_scrollbar_column() {
    let layout = picker_overlay_layout(
        Rect::new(0, 0, 120, 40),
        OverlaySizing {
            has_details: true,
            nav_rows: 40,
        },
    );
    let nav = layout.nav_body_rect();
    assert!(
        nav.width >= 4,
        "nav pane must be wide enough for a scrollbar"
    );
    assert_eq!(nav.y, layout.body_top());
    assert_eq!(nav.height as usize, layout.nav_viewport_rows());
    // Rightmost column of the nav body is where the track is drawn.
    let scrollbar_x = nav.x + nav.width - 1;
    let hit = layout
        .pane_hit(scrollbar_x, nav.y)
        .expect("scrollbar column still belongs to the nav pane");
    assert_eq!(hit.pane, OverlayPane::Nav);
    assert_eq!(hit.pane_row, 0);

    let detail = layout.detail_body_rect().expect("detail pane");
    assert_eq!(detail.y, layout.body_top());
    assert!(detail.x > nav.x + nav.width);
}
