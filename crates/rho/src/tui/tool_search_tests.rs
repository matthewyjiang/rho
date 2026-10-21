use super::*;
use crate::tui::{syntax::MatchQuery, theme::Theme};

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

// Covers: match overlay marks the pattern even without a language highlighter
// Owner: pure unit (grep match highlight)
#[test]
fn match_overlay_without_language() {
    // Styles are derived from the theme at paint time and re-derived at assert
    // time; hold the lock so theme-switching tests cannot flip them in between.
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
