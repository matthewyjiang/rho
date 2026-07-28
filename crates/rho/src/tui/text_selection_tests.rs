use pretty_assertions::assert_eq;
use ratatui::text::Line;

use super::*;

#[test]
fn extracts_forward_selection_across_rendered_lines() {
    let selection = TextSelection {
        anchor: SelectionPosition { line: 4, column: 2 },
        focus: SelectionPosition { line: 5, column: 3 },
    };
    let lines = vec![Line::raw("  alpha   "), Line::raw("beta")];

    assert_eq!(
        selection.selected_text(&lines, 4),
        Some("alpha\nbeta".into())
    );
}

#[test]
fn extracts_backward_selection_in_reading_order() {
    let selection = TextSelection {
        anchor: SelectionPosition { line: 8, column: 4 },
        focus: SelectionPosition { line: 7, column: 2 },
    };
    let lines = vec![Line::raw("  first"), Line::raw("second")];

    assert_eq!(
        selection.selected_text(&lines, 7),
        Some("first\nsecon".into())
    );
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
    let mut in_code_block = false;
    let lines =
        crate::tui::markdown::markdown_lines("```rust\nlet x = 1;\n```", 20, &mut in_code_block);
    let selection = TextSelection {
        anchor: SelectionPosition { line: 0, column: 0 },
        focus: SelectionPosition {
            line: 2,
            column: 19,
        },
    };

    assert_eq!(
        selection.selected_text(&lines, 0),
        Some("╭────────────╮\n│ let x = 1;       │\n╰──────────────────╯".into())
    );
}
