use pretty_assertions::assert_eq;
use ratatui::{layout::Rect, text::Line};

use super::{DragSelection, SelectionBody};

/// One left-button event at a screen cell.
#[derive(Clone, Copy)]
enum Step {
    Press(u16, u16),
    Drag(u16, u16),
    Release(u16, u16),
}

/// A case: name, steps, text copied by each release, selection active after.
struct Case {
    name: &'static str,
    steps: &'static [Step],
    copies: &'static [Option<&'static str>],
    active: bool,
}

// Covers: press/drag/release resolves against the scrolled body. A drag copies
// body lines (not screen rows), dragging past the body clamps to its edge, a
// click or a press outside the body copies nothing and leaves no highlight,
// and a copied span stays highlighted until the next press.
// Owner: pure unit
#[test]
fn drag_selection_copies_body_lines_through_scroll() {
    let lines = (0..10)
        .map(|index| Line::raw(format!("row-{index:02} tail")))
        .collect::<Vec<_>>();
    // Screen rows 5..9 show body lines 3..7 at columns 10..30.
    let body = SelectionBody {
        area: Rect::new(10, 5, 20, 4),
        top_line: 3,
        lines: &lines,
    };
    let cases = [
        Case {
            name: "drag copies scrolled body lines",
            steps: &[Step::Press(10, 5), Step::Drag(13, 6), Step::Release(15, 6)],
            copies: &[Some("row-03 tail\nrow-04")],
            active: true,
        },
        Case {
            name: "release past the body clamps to its edge",
            steps: &[Step::Press(12, 7), Step::Release(50, 20)],
            copies: &[Some("w-05 tail\nrow-06 tail")],
            active: true,
        },
        Case {
            name: "click without movement copies nothing",
            steps: &[Step::Press(12, 6), Step::Release(12, 6)],
            copies: &[None],
            active: false,
        },
        Case {
            name: "press outside the body anchors nothing",
            steps: &[Step::Press(0, 0), Step::Drag(12, 6), Step::Release(15, 7)],
            copies: &[None],
            active: false,
        },
        Case {
            name: "next press drops the copied highlight",
            steps: &[Step::Press(10, 5), Step::Release(15, 6), Step::Press(0, 0)],
            copies: &[Some("row-03 tail\nrow-04")],
            active: false,
        },
    ];
    for Case {
        name,
        steps,
        copies,
        active,
    } in cases
    {
        let mut selection = DragSelection::default();
        let mut copied = Vec::new();
        for &step in steps {
            match step {
                Step::Press(column, row) => selection.press(body, column, row),
                Step::Drag(column, row) => selection.drag(body, column, row),
                Step::Release(column, row) => copied.push(selection.release(body, column, row)),
            }
        }
        let expected = copies
            .iter()
            .copied()
            .map(|copy| copy.map(str::to_owned))
            .collect::<Vec<_>>();
        assert_eq!(
            (copied, selection.is_active()),
            (expected, active),
            "{name}"
        );
    }
}
