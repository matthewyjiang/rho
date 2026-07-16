use super::table::markdown_table_cells;
use super::*;
use ratatui::{style::Modifier, text::Line};

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn renders_markdown_tables_with_alignment_and_inline_styles() {
    let mut in_code_block = false;
    let lines = markdown_lines(
        "| Name | Count | Note |\n| :--- | ---: | :---: |\n| **alpha** | 2 | `ready` |\n| beta | 12 | waiting |",
        40,
        &mut in_code_block,
    );

    assert_eq!(
        lines.iter().map(line_text).collect::<Vec<_>>(),
        vec![
            "┌───────┬───────┬─────────┐",
            "│ Name  │ Count │  Note   │",
            "├───────┼───────┼─────────┤",
            "│ alpha │     2 │  ready  │",
            "│ beta  │    12 │ waiting │",
            "└───────┴───────┴─────────┘",
        ]
    );
    assert!(lines[1]
        .spans
        .iter()
        .any(|span| span.style.has_modifier(Modifier::BOLD)));
    assert!(lines[3]
        .spans
        .iter()
        .any(|span| span.style == Theme::markdown_inline_code()));
}

#[test]
fn wraps_table_cells_to_fit_available_width() {
    let mut in_code_block = false;
    let lines = markdown_lines(
        "| Package | Description |\n| --- | --- |\n| rho | lightweight coding agent |",
        20,
        &mut in_code_block,
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
            "│         │  agent │",
            "└─────────┴────────┘",
        ]
    );
}

#[test]
fn table_parser_preserves_escaped_pipes_and_code_spans() {
    let mut in_code_block = false;
    let lines = markdown_lines(
        "| Expression | Result |\n| --- | --- |\n| a \\| b | `x|y` |",
        30,
        &mut in_code_block,
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
    let mut in_code_block = false;
    let lines = markdown_lines(
        "| Name | Value |\n| --- | --- |\n| rho | agent |\n`a|b`",
        30,
        &mut in_code_block,
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
