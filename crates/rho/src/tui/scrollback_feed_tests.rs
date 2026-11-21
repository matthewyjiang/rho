use pretty_assertions::assert_eq;
use ratatui::layout::Size;

use super::{FeedMark, ResizeRewind, ScrollbackFeed};

// Covers: a resize never moves the mark past rows the terminal may still
// show, so a reflowing terminal's pulled-back rows are repainted rather than
// lost, and rows it pulls back from before the document return to
// scrollback instead of being painted over.
#[test]
fn resize_rewinds_only_for_a_taller_screen() {
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
        // Rows pulled back at a new width no longer line up with the
        // document's, so the mark steps back twice as far.
        (
            "wider and taller",
            100,
            size(80, 24),
            size(100, 30),
            rewind(88, 0),
        ),
        (
            "narrower and taller than twice the mark",
            8,
            size(80, 24),
            size(60, 30),
            rewind(0, 0),
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

// Covers: rows already written can change under the mark. A surface the
// keyboard left is written again from its start, and removed entries step
// the mark back before them, so no changed row is skipped.
#[test]
fn transcript_changes_rewind_the_mark_before_them() {
    let entry = |index, line| FeedMark::Entry { index, line };
    let cases = [
        ("nothing changed", entry(5, 3), None, None, entry(5, 3)),
        (
            "surface at the mark",
            entry(5, 3),
            Some(5),
            None,
            entry(5, 0),
        ),
        (
            "surface before the mark",
            entry(5, 3),
            Some(2),
            None,
            entry(2, 0),
        ),
        (
            "surface not written yet",
            entry(5, 3),
            Some(7),
            None,
            entry(5, 3),
        ),
        (
            "removed at the mark",
            entry(5, 3),
            None,
            Some(5),
            entry(4, usize::MAX),
        ),
        (
            "removed after the mark",
            entry(5, 3),
            None,
            Some(6),
            entry(5, 3),
        ),
        (
            "every entry removed",
            entry(5, 3),
            None,
            Some(0),
            FeedMark::Header(usize::MAX),
        ),
        (
            "removed before a left surface",
            entry(5, 3),
            Some(4),
            Some(2),
            entry(1, usize::MAX),
        ),
        (
            "header only",
            FeedMark::Header(2),
            Some(0),
            Some(0),
            FeedMark::Header(2),
        ),
    ];
    for (name, mark, left_surface, removed_from, expected) in cases {
        assert_eq!(mark.rewound(left_surface, removed_from), expected, "{name}");
    }
}

// Covers: a new width rewraps the entry the mark is in, so a line count from
// the old width would skip its unwritten rows (wider) or repeat many written
// ones (narrower), and a proportional mark skips unwritten rows that rewrap
// less than the written ones.
#[test]
fn a_new_width_never_marks_unwritten_rows_of_the_marked_entry() {
    let entry = |line| FeedMark::Entry { index: 3, line };
    let cases = [
        // 180 long rows halve to 90; 20 short unwritten rows stay 20.
        (
            "wider, short rows unwritten",
            entry(180),
            200,
            110,
            entry(89),
        ),
        ("wider, partly written", entry(180), 200, 100, entry(79)),
        ("narrower, partly written", entry(50), 100, 200, entry(49)),
        ("nothing written", entry(0), 100, 50, entry(0)),
        ("whole entry", entry(200), 200, 100, entry(100)),
        // Rows the entry grew by since the last paint stay unwritten.
        ("grew since the last paint", entry(200), 220, 110, entry(89)),
        ("past the end", entry(usize::MAX), 200, 100, entry(100)),
        ("empty before", entry(0), 0, 10, entry(0)),
        ("header", FeedMark::Header(4), 10, 20, FeedMark::Header(4)),
    ];
    for (name, mark, old_len, new_len, expected) in cases {
        assert_eq!(mark.rewrapped(old_len, new_len), expected, "{name}");
    }
}
