use pretty_assertions::assert_eq;
use ratatui::{buffer::Buffer, layout::Rect, style::Modifier, text::Line};

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
fn copy_notice_reports_the_character_count() {
    let now = Instant::now();

    assert_eq!(CopyNotice::copied(1, now).message(), "1 char copied");
    assert_eq!(CopyNotice::copied(12, now).message(), "12 chars copied");
}

#[test]
fn highlights_selected_screen_cells() {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 3));
    let selection = TextSelection {
        anchor: SelectionPosition {
            line: 10,
            column: 2,
        },
        focus: SelectionPosition {
            line: 11,
            column: 3,
        },
    };

    highlight_selection(&mut buffer, Rect::new(0, 0, 8, 3), 10, selection);

    assert!(buffer[(1, 0)].modifier.is_empty());
    assert!(buffer[(2, 0)].modifier.contains(Modifier::REVERSED));
    assert!(buffer[(7, 0)].modifier.contains(Modifier::REVERSED));
    assert!(buffer[(0, 1)].modifier.contains(Modifier::REVERSED));
    assert!(buffer[(3, 1)].modifier.contains(Modifier::REVERSED));
    assert!(buffer[(4, 1)].modifier.is_empty());
    assert!(buffer[(0, 2)].modifier.is_empty());
}
