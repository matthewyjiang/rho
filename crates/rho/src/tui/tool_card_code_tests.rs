use pretty_assertions::assert_eq;

use super::{render_code_body, ToolBodyWindow};

// Covers: a tail window clips the wrong end of a wrapped source row, including
// UTF-8, or consumes more than the existing terminal-row budget.
// Owner: pure code-body wrap/window math; PTY owns model-generation visibility.
#[test]
fn code_window_uses_terminal_rows_and_preserves_full_height() {
    for (source, window, budget, expected) in [
        ("abcdef", ToolBodyWindow::Head, 1, vec!["    abcd"]),
        ("abcdef", ToolBodyWindow::Tail, 1, vec!["    ef  "]),
        ("αβγδεζ", ToolBodyWindow::Tail, 1, vec!["    εζ  "]),
        (
            "abcdef",
            ToolBodyWindow::Tail,
            usize::MAX,
            vec!["    abcd", "    ef  "],
        ),
    ] {
        let mut remaining = budget;
        let rendered = render_code_body(
            &[source.into()],
            "python",
            window,
            /*width*/ 8,
            &mut remaining,
        );
        let rows: Vec<String> = rendered
            .groups
            .into_iter()
            .flat_map(super::super::ChildGroup::into_lines)
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(
            (rows, rendered.total_rows),
            (
                expected
                    .iter()
                    .map(|row| row.to_string())
                    .collect::<Vec<_>>(),
                2
            ),
            "{window:?}: {source}"
        );
    }
}
