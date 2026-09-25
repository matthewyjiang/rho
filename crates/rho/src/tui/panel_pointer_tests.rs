use crossterm::event::{MouseButton, MouseEventKind};
use pretty_assertions::assert_eq;
use ratatui::{layout::Rect, text::Line};

use super::{PanelPointer, PanelPointerEffect};
use crate::tui::{
    copy_interaction::CopyHit,
    overlay_panel::{render_overlay_panel, OverlayPanelFrame},
};

const DOWN: MouseEventKind = MouseEventKind::Down(MouseButton::Left);
const DRAG: MouseEventKind = MouseEventKind::Drag(MouseButton::Left);
const UP: MouseEventKind = MouseEventKind::Up(MouseButton::Left);

/// 40 rows of `row-NN tail` in a 40x14 area: 8 body rows, so the body
/// overflows, the scrollbar shows, and the last top line is 32.
fn frame(scroll: usize) -> OverlayPanelFrame {
    let body = (0..40)
        .map(|index| Line::raw(format!("row-{index:02} tail")))
        .collect();
    render_overlay_panel("Title", "Esc close", body, scroll, Rect::new(0, 0, 40, 14))
}

// Covers: dragging the panel scrollbar thumb maps the pointer row to a top
// line at both ends, the middle, and past the track, so the thumb follows the
// pointer instead of jumping.
// Owner: pure unit
#[test]
fn scrollbar_drag_maps_pointer_rows_to_top_lines() {
    let frame = frame(/*scroll*/ 0);
    let track = frame
        .scrollbar()
        .expect("overflowing body shows a scrollbar")
        .rect;
    let body = frame.body();
    assert_eq!(track, Rect::new(body.right(), body.y, 1, body.height));

    let mut pointer = PanelPointer::default();
    // Grab the thumb at its top row; the grab offset stays zero.
    assert_eq!(
        pointer.handle(DOWN, track.x, track.y, &frame),
        PanelPointerEffect::ScrollTo(0)
    );
    let cases = [
        ("middle", track.y + 3, 16),
        ("bottom", track.bottom() - 1, 32),
        ("past the track", track.bottom() + 5, 32),
        ("top", track.y, 0),
    ];
    for (name, row, top_line) in cases {
        assert_eq!(
            pointer.handle(DRAG, track.x, row, &frame),
            PanelPointerEffect::ScrollTo(top_line),
            "{name}"
        );
    }
    assert_eq!(
        pointer.handle(UP, track.x, track.y, &frame),
        PanelPointerEffect::None
    );
    // Release ends the drag: a later drag selects nothing and scrolls nothing.
    assert_eq!(
        pointer.handle(DRAG, track.x, track.bottom() - 1, &frame),
        PanelPointerEffect::None
    );
}

/// One pointer event in a test sequence: its kind and body column.
type PointerStep = (MouseEventKind, u16);

// Covers: body presses resolve against the scrolled viewport. A drag copies
// the body line under the pointer (not the screen row), a copy target copies
// its payload instead of starting a selection, and a click copies nothing.
// Owner: pure unit
#[test]
fn body_press_routes_to_selection_or_copy_target_through_scroll() {
    let mut scrolled = frame(/*scroll*/ 10);
    let body = scrolled.body();
    // Body line 11 is the second visible row once scrolled by 10.
    scrolled.copy_hits.push(CopyHit {
        row: 11,
        columns: 8..12,
        text: "payload".into(),
    });
    let row = body.y + 1;
    // (name, pointer events as (kind, body column), expected effect of the last)
    let cases: [(&str, &[PointerStep], PanelPointerEffect); 3] = [
        (
            "drag copies the scrolled line",
            &[(DOWN, 0), (DRAG, 3), (UP, 5)],
            PanelPointerEffect::Copy("row-11".into()),
        ),
        (
            "copy target copies on press",
            &[(DOWN, 9)],
            PanelPointerEffect::Copy("payload".into()),
        ),
        (
            "click without movement copies nothing",
            &[(DOWN, 2), (UP, 2)],
            PanelPointerEffect::None,
        ),
    ];
    for (name, events, expected) in cases {
        let mut pointer = PanelPointer::default();
        let mut last = PanelPointerEffect::None;
        for &(kind, column) in events {
            last = pointer.handle(kind, body.x + column, row, &scrolled);
        }
        assert_eq!(last, expected, "{name}");
    }
}
