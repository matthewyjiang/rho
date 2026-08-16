use ratatui::layout::Rect;

use super::*;

#[test]
fn hides_when_history_fits_viewport() {
    assert_eq!(HistoryScrollbar::new(Rect::new(0, 0, 40, 10), 10, 0), None);
    assert_eq!(HistoryScrollbar::new(Rect::new(0, 0, 40, 1), 20, 0), None);
}

#[test]
fn anchors_to_history_right_edge() {
    let scrollbar = HistoryScrollbar::new(Rect::new(2, 3, 40, 10), 20, 5).unwrap();

    assert_eq!(scrollbar.rect, Rect::new(41, 3, 1, 10));
    assert!(scrollbar.contains(41, 3));
    assert!(scrollbar.contains(41, 12));
    assert!(!scrollbar.contains(40, 3));
    assert!(!scrollbar.contains(41, 13));
}

#[test]
fn maps_pointer_rows_to_scroll_positions() {
    let scrollbar = HistoryScrollbar::new(Rect::new(0, 0, 40, 10), 100, 0).unwrap();
    let drag = HistoryScrollbarDrag::Track {
        thumb_grab_offset: 0,
    };

    assert_eq!(scrollbar.top_line_for_pointer(0, drag), 0);
    assert_eq!(scrollbar.top_line_for_pointer(5, drag), 50);
    assert_eq!(scrollbar.top_line_for_pointer(9, drag), 90);
}

#[test]
fn dragging_thumb_does_not_jump_on_mouse_down() {
    let scrollbar = HistoryScrollbar::new(Rect::new(0, 0, 40, 10), 100, 45).unwrap();
    let drag = scrollbar.begin_drag(5);

    assert_eq!(
        drag,
        HistoryScrollbarDrag::Thumb {
            thumb_grab_offset: 0,
            start_row: 5,
            start_top_line: 45,
        }
    );
    assert_eq!(scrollbar.top_line_for_pointer(5, drag), 45);
    assert_eq!(scrollbar.top_line_for_pointer(6, drag), 60);
}

#[test]
fn clicking_track_centers_thumb_on_pointer() {
    let scrollbar = HistoryScrollbar::new(Rect::new(0, 0, 40, 10), 100, 0).unwrap();
    let drag = scrollbar.begin_drag(9);

    assert_eq!(
        drag,
        HistoryScrollbarDrag::Track {
            thumb_grab_offset: 0
        }
    );
    assert_eq!(scrollbar.top_line_for_pointer(9, drag), 90);
}

#[test]
fn bottom_position_uses_bottom_scroll_state() {
    assert_eq!(
        scroll_state_for_top_line(100, 10, 90),
        HistoryScroll::Bottom
    );
    assert_eq!(
        scroll_state_for_top_line(100, 10, 200),
        HistoryScroll::Bottom
    );
    assert_eq!(
        scroll_state_for_top_line(100, 10, 89),
        HistoryScroll::Manual { top_line: 89 }
    );
}

// Covers: wheel ticks at the measured body start must still request earlier rows.
// Owner: history scroll range
#[test]
fn unmeasured_prefix_scroll_need_triggers_at_measured_body() {
    let cases = [
        (
            /*start*/ 80, /*delta*/ -3, /*unmeasured*/ true, /*need*/ 0,
        ),
        (0, -3, true, 3),
        (2, -5, true, 3),
        (12, -3, false, 0),
        (12, 3, true, 0),
    ];
    for (start, delta, has_unmeasured_prefix, expected) in cases {
        assert_eq!(
            unmeasured_prefix_scroll_need(start, delta, has_unmeasured_prefix),
            expected,
            "start={start} delta={delta} unmeasured={has_unmeasured_prefix}"
        );
    }
}

#[test]
fn scroll_chrome_scroll_by_clamps_to_bottom_without_auto_reveal() {
    use std::time::{Duration, Instant};

    let mut chrome = HistoryScrollChrome::default();
    let now = Instant::now();
    chrome.scroll_by(100, 10, -5);
    assert_eq!(chrome.scroll(), HistoryScroll::Manual { top_line: 85 });
    assert!(!chrome.should_render(now));
    chrome.reveal(now, Duration::from_millis(1200));
    assert!(chrome.should_render(now));

    chrome.scroll_by(100, 10, 20);
    assert_eq!(chrome.scroll(), HistoryScroll::Bottom);
    assert!(!chrome.should_render(now));
}
