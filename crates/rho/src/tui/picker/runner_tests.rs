use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEventKind};
use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::{
    super::{
        overlay_layout::{picker_overlay_layout, OverlayPane},
        PickerAction, PickerItem, PickerLayout, UiPicker,
    },
    apply_mouse, MouseEffect, PointerInput,
};

const AREA: Rect = Rect::new(0, 0, 80, 24);

fn overlay_picker(count: usize) -> UiPicker {
    let items = (0..count)
        .map(|index| PickerItem {
            section: None,
            label: format!("model {index:02}"),
            detail: None,
            preview: None,
            badge: None,
            value: format!("value {index:02}"),
            selection_verb: None,
            allow_filter_completion: true,
        })
        .collect();
    UiPicker::new("models", items, PickerAction::Config).with_layout(PickerLayout::Overlay)
}

/// Screen cell of nav pane row `pane_row`, found through the same layout the
/// runner hit-tests against.
fn nav_cell(picker: &UiPicker, pane_row: usize) -> (u16, u16) {
    let layout = picker_overlay_layout(AREA, picker.overlay_sizing());
    let column = layout.outer.x + 2;
    (layout.outer.top()..layout.outer.bottom())
        .find(|&row| {
            layout
                .pane_hit(column, row)
                .is_some_and(|hit| hit.pane == OverlayPane::Nav && hit.pane_row == pane_row)
        })
        .map(|row| (column, row))
        .expect("nav row is painted")
}

fn event(kind: MouseEventKind, (column, row): (u16, u16), now: Instant) -> PointerInput {
    PointerInput {
        kind,
        column,
        row,
        now,
    }
}

// Covers: the standalone runner maps clicks, double clicks, hover, and the
// wheel onto the rows the overlay painted: one click selects, a second click
// on the same row within the gap submits like Enter, a slow second click or
// a click on another row only selects, and the wheel scrolls the nav window
// without moving the selection.
// Owner: standalone picker pointer mapping (pure geometry).
#[test]
fn pointer_selects_submits_hovers_and_scrolls_nav_rows() {
    let mut picker = overlay_picker(60);
    let mut last_click = None;
    let start = Instant::now();
    let down = MouseEventKind::Down(MouseButton::Left);
    let row_two = nav_cell(&picker, 2);
    let row_three = nav_cell(&picker, 3);

    let hover = event(MouseEventKind::Moved, row_three, start);
    assert_eq!(
        apply_mouse(&mut picker, AREA, hover, &mut last_click),
        MouseEffect::None
    );
    assert_eq!(picker.hovered_nav_row(), Some(3));

    // (press cell, time offset, expected effect, expected selection)
    let presses = [
        (row_two, 0, MouseEffect::None, 2),
        // Too slow for a double click: just selects again.
        (row_two, 900, MouseEffect::None, 2),
        // Different row inside the gap: selects, no submit.
        (row_three, 1_000, MouseEffect::None, 3),
        (row_three, 1_200, MouseEffect::Submit, 3),
        // The submit consumed the sequence; a third press starts over.
        (row_three, 1_300, MouseEffect::None, 3),
    ];
    for (cell, offset, effect, selected) in presses {
        let now = start + Duration::from_millis(offset);
        assert_eq!(
            apply_mouse(&mut picker, AREA, event(down, cell, now), &mut last_click),
            effect,
            "press at {offset}ms"
        );
        assert_eq!(picker.selected, selected, "press at {offset}ms");
    }

    let nav_rows = picker_overlay_layout(AREA, picker.overlay_sizing())
        .scroll_targets()
        .nav_rows;
    let top = picker.nav_window_start(nav_rows);
    apply_mouse(
        &mut picker,
        AREA,
        event(MouseEventKind::ScrollDown, row_two, start),
        &mut last_click,
    );
    assert_eq!(
        picker.nav_window_start(nav_rows),
        top + crate::tui::HISTORY_MOUSE_SCROLL_LINES
    );
    assert_eq!(picker.selected, 3, "the wheel never moves the selection");
    // A click after scrolling selects the row now painted under the pointer.
    apply_mouse(
        &mut picker,
        AREA,
        event(down, row_two, start),
        &mut last_click,
    );
    assert_eq!(
        picker.selected,
        top + crate::tui::HISTORY_MOUSE_SCROLL_LINES + 2
    );
}
