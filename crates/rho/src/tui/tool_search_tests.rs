use super::*;
use crate::tui::{
    syntax::MatchQuery,
    theme::{SyntaxRole, Theme},
};

fn query(pattern: &str) -> MatchQuery {
    MatchQuery::new(
        pattern, /*literal*/ false, /*case_sensitive*/ true,
    )
}

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
    crate::tui::syntax::warm_syntax_set();
    // Styles are derived from the theme at paint time and re-derived at assert
    // time; hold the lock so theme-switching tests cannot flip them in between.
    let _guard = crate::tui::theme::theme_test_lock();
    let mut syntax = SearchSyntax::new(query("answer"));
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
    // Styles are derived from the theme at paint time and re-derived at assert
    // time; hold the lock so theme-switching tests cannot flip them in between.
    let _guard = crate::tui::theme::theme_test_lock();
    let mut syntax = SearchSyntax::new(query("needle"));
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

// Covers: literal overlay must not treat regex metacharacters as wildcards
// Owner: pure unit (grep match semantics)
#[test]
fn literal_dot_does_not_match_every_character() {
    // Styles are derived from the theme at paint time and re-derived at assert
    // time; hold the lock so theme-switching tests cannot flip them in between.
    let _guard = crate::tui::theme::theme_test_lock();
    let mut syntax = SearchSyntax::new(MatchQuery::new(
        ".", /*literal*/ true, /*case_sensitive*/ true,
    ));
    let mut lines = Vec::new();
    syntax.paint_line("notes.txt", 80, &mut lines);
    lines.clear();
    syntax.paint_line("1 | a.b", 80, &mut lines);
    let body = &lines[0];
    let match_spans: Vec<_> = body
        .spans
        .iter()
        .filter(|span| span.style == Theme::search_match(Theme::text()))
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(
        match_spans,
        vec!["."],
        "literal '.' should match only the dot"
    );
}

// Covers: case-sensitive overlay must not highlight differently-cased text
// Owner: pure unit (grep match semantics)
#[test]
fn case_sensitive_pattern_skips_different_case() {
    // Styles are derived from the theme at paint time and re-derived at assert
    // time; hold the lock so theme-switching tests cannot flip them in between.
    let _guard = crate::tui::theme::theme_test_lock();
    let mut syntax = SearchSyntax::new(MatchQuery::new(
        "foo", /*literal*/ false, /*case_sensitive*/ true,
    ));
    let mut lines = Vec::new();
    syntax.paint_line("notes.txt", 80, &mut lines);
    lines.clear();
    syntax.paint_line("1 | FOO foo", 80, &mut lines);
    let body = &lines[0];
    let match_spans: Vec<_> = body
        .spans
        .iter()
        .filter(|span| span.style == Theme::search_match(Theme::text()))
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(match_spans, vec!["foo"]);
}
