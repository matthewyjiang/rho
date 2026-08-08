use super::*;
use crate::tui::theme::{SyntaxRole, Theme};

// Covers: content-mode path headers and N | lines classify correctly
// Owner: pure unit (grep body parse)
#[test]
fn classifies_path_and_content_rows() {
    assert!(matches!(
        classify_search_line("src/lib.rs"),
        SearchLine::Path {
            path: "src/lib.rs",
            ..
        }
    ));
    assert!(matches!(
        classify_search_line("[src/lib.rs#AB12]"),
        SearchLine::Path {
            path: "src/lib.rs",
            ..
        }
    ));
    match classify_search_line("12 | let answer = 42;") {
        SearchLine::Content { prefix, source } => {
            assert_eq!(prefix, "12 | ");
            assert_eq!(source, "let answer = 42;");
        }
        other => panic!("expected content, got {other:?}"),
    }
    assert!(matches!(
        classify_search_line("... +3 more in this file"),
        SearchLine::Meta
    ));
    assert!(matches!(
        classify_search_line("5 matches in 2 files"),
        SearchLine::Meta
    ));
}

// Covers: grep body paints rust keywords from the path header
// Owner: pure unit (grep language highlight)
#[test]
fn paints_rust_from_path_header() {
    let mut syntax = SearchSyntax::new(Some("answer"));
    let mut lines = Vec::new();
    syntax.paint_line("src/main.rs", 80, &mut lines);
    lines.clear();
    syntax.paint_line("1 | let answer = 42; // note", 80, &mut lines);
    let body = &lines[0];
    assert!(
        body.spans.iter().any(|span| {
            span.content.contains("let") && span.style == Theme::syntax(SyntaxRole::Keyword)
        }),
        "expected keyword highlight: {:?}",
        body.spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect::<Vec<_>>()
    );
    assert!(
        body.spans.iter().any(|span| {
            span.content.as_ref() == "answer" && span.style == Theme::search_match(Theme::text())
        }),
        "expected match overlay on answer: {:?}",
        body.spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect::<Vec<_>>()
    );
}

// Covers: match overlay marks the pattern even without a language highlighter
// Owner: pure unit (grep match highlight)
#[test]
fn match_overlay_without_language() {
    let mut syntax = SearchSyntax::new(Some("needle"));
    let mut lines = Vec::new();
    syntax.paint_line("notes.txt", 80, &mut lines);
    lines.clear();
    syntax.paint_line("3 | find the needle here", 80, &mut lines);
    let body = &lines[0];
    let match_span = body
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "needle")
        .expect("needle span");
    assert_eq!(
        match_span.style,
        Theme::search_match(Theme::text()),
        "match should use search_match style"
    );
}
