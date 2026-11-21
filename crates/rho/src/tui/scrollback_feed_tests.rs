use pretty_assertions::assert_eq;
use ratatui::layout::Size;

use super::{ResizeRewind, ScrollbackFeed};

// Covers: a resize never moves the mark past rows the terminal may still
// show, so a reflowing terminal's pulled-back rows are repainted rather than
// lost, and rows it pulls back from before the document return to
// scrollback instead of being painted over.
#[test]
fn resize_rewinds_only_for_a_taller_screen_of_the_same_width() {
    let size = |width, height| Size { width, height };
    let rewind = |mark_row, foreign_rows| ResizeRewind {
        mark_row,
        foreign_rows,
    };
    let cases = [
        ("taller", 100, size(80, 24), size(80, 30), rewind(94, 0)),
        (
            "taller than the mark",
            4,
            size(80, 24),
            size(80, 30),
            rewind(0, 2),
        ),
        (
            "taller before any write",
            0,
            size(80, 24),
            size(80, 30),
            rewind(0, 6),
        ),
        ("shorter", 100, size(80, 30), size(80, 24), rewind(100, 0)),
        ("narrower", 100, size(80, 24), size(60, 24), rewind(100, 0)),
        (
            "wider and taller",
            100,
            size(80, 24),
            size(100, 30),
            rewind(100, 0),
        ),
    ];
    for (name, mark_row, previous, resized, expected) in cases {
        assert_eq!(
            ScrollbackFeed::rewind_after_resize(mark_row, previous, resized),
            expected,
            "{name}"
        );
    }
}
