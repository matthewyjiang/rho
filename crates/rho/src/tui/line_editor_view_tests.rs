use pretty_assertions::assert_eq;

use super::{editor_viewport, EditorPresentation, EditorViewport, EditorWindow};

// Covers: cropping wide/combined glyphs must not split a grapheme or put the
// caret outside the field. Owner: pure viewport math; PTY cannot enumerate
// zero-cell fields or carets/window starts within multi-scalar graphemes reliably.
#[test]
fn viewport_crops_at_grapheme_boundaries_and_uses_terminal_cells() {
    let value = "a界e\u{301}👩\u{200d}💻z";
    for (cursor, width, initial_start, visible, cursor_column, start) in [
        (0, 0, 0, "", 0, 0),
        (0, 2, 0, "a", 0, 0),
        (2, 1, 0, "e\u{301}", 0, 2),
        (3, 4, 0, "界e\u{301}", 3, 1),
        (1, 2, 0, "界", 0, 1),
        (4, 4, 0, "e\u{301}👩\u{200d}💻z", 1, 2),
        (5, 4, 0, "e\u{301}👩\u{200d}💻z", 3, 2),
        (7, 4, 0, "e\u{301}👩\u{200d}💻z", 3, 2),
        (8, 4, 0, "👩\u{200d}💻z", 3, 4),
        (8, 3, 0, "z", 1, 7),
        (8, 1, 0, "", 0, 8),
        (3, 4, 3, "e\u{301}👩\u{200d}💻z", 1, 2),
        (5, 4, 5, "👩\u{200d}💻z", 2, 4),
        (8, 4, 15, "👩\u{200d}💻z", 3, 4),
    ] {
        assert_eq!(
            editor_viewport(
                value,
                cursor,
                EditorPresentation::Plain,
                width,
                EditorWindow {
                    start: initial_start
                },
            ),
            EditorViewport {
                value: visible.to_owned(),
                cursor_column,
                window: EditorWindow { start },
            },
            "cursor={cursor}, width={width}, start={initial_start}",
        );
    }
}
