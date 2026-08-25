use super::table::markdown_table_cells;
use super::*;
use ratatui::text::Line;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn wraps_table_cells_to_fit_available_width() {
    let mut fence_state = CodeFenceState::default();
    let lines = markdown_lines(
        "| Package | Description |\n| --- | --- |\n| rho | lightweight coding agent |",
        20,
        &mut fence_state,
    );

    assert!(lines
        .iter()
        .all(|line| display_width(&line_text(line)) <= 20));
    assert_eq!(
        lines.iter().map(line_text).collect::<Vec<_>>(),
        vec![
            "┌─────────┬────────┐",
            "│ Package │ Descri │",
            "│         │ ption  │",
            "├─────────┼────────┤",
            "│ rho     │ lightw │",
            "│         │ eight  │",
            "│         │ coding │",
            "│         │ agent  │",
            "└─────────┴────────┘",
        ]
    );
}

#[test]
fn table_parser_preserves_escaped_pipes_and_code_spans() {
    let mut fence_state = CodeFenceState::default();
    let lines = markdown_lines(
        "| Expression | Result |\n| --- | --- |\n| a \\| b | `x|y` |",
        30,
        &mut fence_state,
    );

    assert!(lines.iter().any(|line| line_text(line).contains("a | b")));
    assert!(lines.iter().any(|line| line_text(line).contains("x|y")));
}

#[test]
fn table_parser_preserves_an_escaped_trailing_pipe_without_a_border() {
    assert_eq!(
        markdown_table_cells("A | B\\|"),
        vec!["A".to_string(), "B|".to_string()]
    );
}

#[test]
fn table_parser_stops_before_lines_with_only_protected_pipes() {
    let mut fence_state = CodeFenceState::default();
    let lines = markdown_lines(
        "| Name | Value |\n| --- | --- |\n| rho | agent |\n`a|b`",
        30,
        &mut fence_state,
    );

    assert_eq!(
        lines.iter().map(line_text).collect::<Vec<_>>(),
        vec![
            "┌──────┬───────┐",
            "│ Name │ Value │",
            "├──────┼───────┤",
            "│ rho  │ agent │",
            "└──────┴───────┘",
            "a|b",
        ]
    );
}

#[test]
fn table_parser_preserves_pipes_in_multi_backtick_code_spans() {
    assert_eq!(
        markdown_table_cells("| Example | Result |\n"),
        vec!["Example".to_string(), "Result".to_string()]
    );
    assert_eq!(
        markdown_table_cells("| ``x|y`` | ok |"),
        vec!["``x|y``".to_string(), "ok".to_string()]
    );
    assert_eq!(
        markdown_table_cells("| `x | y |"),
        vec!["`x".to_string(), "y".to_string()]
    );
}

#[test]
fn lone_pipe_line_parses_as_a_single_empty_cell() {
    assert_eq!(markdown_table_cells("|"), vec![String::new()]);
}

#[test]
fn partial_separator_row_is_not_a_table() {
    assert_eq!(
        super::table::markdown_table_line_count(&["| a | b |", "|"]),
        None
    );
}

#[test]
fn lone_pipe_body_row_does_not_panic() {
    super::table::markdown_table_line_count(&["| a | b |", "| - | - |", "|"]);
}

// Covers: frozen streaming widths reject a later cell that would reflow.
// Owner: markdown table streaming append
#[test]
fn streaming_table_rejects_a_row_that_needs_reflow() {
    let table = streaming_table("| A | B |\n| --- | --- |\n| x | y |\n", 40).expect("table");
    assert!(table.paint_data_row("| x | y |").is_some());
    assert!(table.paint_data_row("| much-longer-cell | z |").is_none());
    assert!(table.paint_data_row("After the table").is_none());
}
