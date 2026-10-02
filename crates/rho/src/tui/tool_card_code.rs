//! Code-body windows measured in wrapped terminal rows, not source rows.

use ratatui::text::Line;

use crate::app::interactive_presenter::ToolBodyWindow;

use super::ChildGroup;
use crate::tui::tool_code::CodeSyntax;

pub(super) struct CodeBodyRender {
    pub(super) groups: Vec<ChildGroup>,
    pub(super) total_rows: usize,
}

/// Paint only the selected window. Advance syntax through skipped source so
/// strings spanning the hidden prefix still highlight correctly in the tail.
/// Highlighting retains CodeSyntax's existing work/line-byte budgets.
pub(super) fn render_code_body(
    lines: &[String],
    language: &str,
    window: ToolBodyWindow,
    width: usize,
    paint_remaining: &mut usize,
) -> CodeBodyRender {
    let logical: Vec<_> = lines
        .iter()
        .flat_map(|line| line.lines().chain(line.is_empty().then_some("")))
        .collect();
    let row_counts: Vec<_> = logical
        .iter()
        .map(|line| CodeSyntax::estimate_rows(language, line, width))
        .collect();
    let total_rows = row_counts
        .iter()
        .copied()
        .fold(0usize, usize::saturating_add);
    let mut skip = match window {
        ToolBodyWindow::Head => 0,
        ToolBodyWindow::Tail => total_rows.saturating_sub(*paint_remaining),
    };
    let mut syntax = CodeSyntax::new(language);
    let mut groups = Vec::new();
    for (line, rows) in logical.iter().zip(row_counts) {
        if *paint_remaining == 0 {
            break;
        }
        if skip >= rows {
            skip -= rows;
            syntax.advance_line(line);
            continue;
        }
        let mut painted: Vec<Line<'static>> = Vec::new();
        syntax.paint_line(line, width, &mut painted);
        if skip > 0 {
            painted.drain(..skip);
            skip = 0;
        }
        painted.truncate(*paint_remaining);
        *paint_remaining -= painted.len();
        groups.push(ChildGroup::Plain(painted));
    }
    CodeBodyRender { groups, total_rows }
}

#[cfg(test)]
#[path = "tool_card_code_tests.rs"]
mod tests;
