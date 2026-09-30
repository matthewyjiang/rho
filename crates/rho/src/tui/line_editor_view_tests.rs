use pretty_assertions::assert_eq;

use super::{editor_viewport, EditorViewport};

// Covers: cropping wide/combined glyphs must not split a grapheme or put the
// caret outside the field. Owner: pure viewport math; PTY cannot enumerate
// zero-cell fields or carets within multi-scalar graphemes reliably.
#[test]
fn viewport_crops_at_grapheme_boundaries_and_uses_terminal_cells() {
    let value = "a界e\u{301}👩\u{200d}💻z";
    for (cursor, width, visible, cursor_column) in [
        (0, 0, "", 0),
        (0, 2, "a", 0),
        (2, 1, "e\u{301}", 0),
        (3, 4, "界e\u{301}", 3),
        (4, 4, "界e\u{301}", 3),
        (5, 4, "e\u{301}👩\u{200d}💻z", 3),
        (7, 4, "e\u{301}👩\u{200d}💻z", 3),
        (8, 4, "👩\u{200d}💻z", 3),
        (8, 3, "z", 1),
        (8, 1, "", 0),
    ] {
        assert_eq!(
            editor_viewport(value, cursor, width),
            EditorViewport {
                value: visible,
                cursor_column,
            },
            "cursor={cursor}, width={width}",
        );
    }
}
