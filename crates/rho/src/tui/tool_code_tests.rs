use pretty_assertions::assert_eq;

use super::CodeSyntax;
use crate::tui::{
    syntax::warm_syntax_set,
    theme::{SyntaxRole, Theme},
};

fn styled(lines: &[ratatui::text::Line<'static>]) -> Vec<(String, ratatui::style::Style)> {
    lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| (span.content.to_string(), span.style))
        .collect()
}

// Covers: a code body carries highlighter state across lines, so a keyword on
// the second line is painted and an unknown language stays plain.
// Owner: pure TUI (code body paint)
#[test]
fn code_lines_highlight_by_language_and_keep_state() {
    warm_syntax_set();
    let _guard = crate::tui::theme::theme_test_lock();
    Theme::apply_committed("terminal");
    let keyword = Theme::syntax(SyntaxRole::Keyword);

    let mut python = CodeSyntax::new("python");
    let mut lines = Vec::new();
    python.paint_line("x = 1", 40, &mut lines);
    python.paint_line("for n in range(x):", 40, &mut lines);
    assert!(
        styled(&lines)
            .iter()
            .any(|(text, style)| text == "for" && style.fg == keyword.fg),
        "expected a highlighted `for`: {:?}",
        styled(&lines)
    );

    let mut unknown = CodeSyntax::new("not-a-language");
    let mut plain = Vec::new();
    unknown.paint_line("for n in x", 40, &mut plain);
    assert_eq!(
        styled(&plain)
            .iter()
            .filter(|(text, _)| text.contains("for"))
            .map(|(_, style)| *style)
            .collect::<Vec<_>>(),
        vec![Theme::text()]
    );
}

// Covers: long code lines wrap between tokens, keeping their highlight, and a
// row estimate without paint agrees with the painted row count.
// Owner: pure TUI (code body paint)
#[test]
fn code_lines_wrap_between_tokens() {
    warm_syntax_set();
    let line = "print(\"codemode fixture batch\", len(hits))";
    let mut syntax = CodeSyntax::new("python");
    let mut lines = Vec::new();
    // Indent "    " is 4 cols; content width 20.
    let rows = syntax.paint_line(line, 24, &mut lines);
    let text: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    assert_eq!(
        text,
        [
            "    print(\"codemode",
            "    fixture batch\",",
            "    len(hits))",
        ]
    );
    assert_eq!(rows, CodeSyntax::estimate_rows(line, 24));
}
