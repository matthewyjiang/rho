use pretty_assertions::assert_eq;
use ratatui::text::Line;

use super::*;

#[test]
fn extracts_multiline_selection_in_reading_order() {
    for (case, anchor, focus, lines, first_line, expected) in [
        (
            "forward",
            SelectionPosition { line: 4, column: 2 },
            SelectionPosition { line: 5, column: 3 },
            [Line::raw("  alpha   "), Line::raw("beta")],
            4,
            "alpha\nbeta",
        ),
        (
            "backward",
            SelectionPosition { line: 8, column: 4 },
            SelectionPosition { line: 7, column: 2 },
            [Line::raw("  first"), Line::raw("second")],
            7,
            "first\nsecon",
        ),
    ] {
        let selection = TextSelection { anchor, focus };
        assert_eq!(
            selection.selected_text(&lines, first_line),
            Some(expected.into()),
            "{case}"
        );
    }
}

#[test]
fn selecting_any_cell_of_a_wide_grapheme_copies_the_whole_grapheme() {
    let selection = TextSelection {
        anchor: SelectionPosition { line: 0, column: 1 },
        focus: SelectionPosition { line: 0, column: 2 },
    };
    let lines = vec![Line::raw("a🙂b")];

    assert_eq!(selection.selected_text(&lines, 0), Some("🙂".into()));
}

#[test]
fn click_without_drag_does_not_copy() {
    let selection = TextSelection::new(SelectionPosition { line: 0, column: 0 });

    assert_eq!(selection.selected_text(&[Line::raw("text")], 0), None);
}

#[test]
fn excludes_code_block_copy_button_from_drag_selection() {
    // The rendered COPY span is matched against the theme's copy-button style,
    // so a concurrent theme switch would rewrite one side of that comparison.
    let _theme = crate::tui::theme::theme_test_lock();
    let mut fence_state = crate::tui::markdown::CodeFenceState::default();
    let lines =
        crate::tui::markdown::markdown_lines("```rust\nlet x = 1;\n```", 20, &mut fence_state);
    let selection = TextSelection {
        anchor: SelectionPosition { line: 0, column: 0 },
        focus: SelectionPosition {
            line: 1,
            column: 19,
        },
    };

    assert_eq!(
        selection.selected_text(&lines, 0),
        Some("RUST\nlet x = 1;".into())
    );
}
