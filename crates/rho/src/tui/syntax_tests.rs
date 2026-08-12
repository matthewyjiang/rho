use super::*;
use crate::tui::theme::{SyntaxRole, Theme};

fn segment_texts(segments: &[HighlightSegment]) -> Vec<&str> {
    segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect()
}

fn role_of(segments: &[HighlightSegment], text: &str) -> Option<SyntaxRole> {
    segments
        .iter()
        .find(|segment| segment.text.trim() == text)
        .unwrap_or_else(|| panic!("no segment for {text:?} in {segments:?}"))
        .role
}

// Covers: rust fence tokens map onto distinct syntax roles
// Owner: pure unit (syntax highlight)
#[test]
fn rust_tokens_map_to_distinct_roles() {
    let mut highlighter = BlockHighlighter::for_language("rust").expect("bundled rust syntax");
    let segments = highlighter.highlight_line("let answer = 42; // note");

    assert_eq!(
        segment_texts(&segments).concat(),
        "let answer = 42; // note"
    );
    assert_eq!(role_of(&segments, "let"), Some(SyntaxRole::Keyword));
    assert_eq!(role_of(&segments, "42"), Some(SyntaxRole::Constant));
    assert_eq!(role_of(&segments, "// note"), Some(SyntaxRole::Comment));
}

// Covers: multi-line string grammar state survives across highlight_line calls
// Owner: pure unit (syntax highlight)
#[test]
fn string_state_carries_across_lines() {
    let mut highlighter = BlockHighlighter::for_language("rust").expect("bundled rust syntax");
    highlighter.highlight_line("let text = \"open");
    let segments = highlighter.highlight_line("still inside");

    assert!(segments
        .iter()
        .all(|segment| segment.role == Some(SyntaxRole::String)));
}

// Covers: unknown fence language falls back to no highlighter
// Owner: pure unit (syntax language lookup)
#[test]
fn unknown_language_has_no_highlighter() {
    assert!(BlockHighlighter::for_language("no-such-language").is_none());
}

// Covers: TypeScript fence tags resolve after two-face syntax dump
// Owner: pure unit (syntax language lookup)
#[test]
fn typescript_fence_tokens_resolve() {
    for token in ["ts", "tsx", "typescript"] {
        assert!(
            BlockHighlighter::for_language(token).is_some(),
            "expected highlighter for fence token {token}"
        );
    }
}

// Covers: common alias tags map onto dump-native grammars
// Owner: pure unit (syntax language lookup)
#[test]
fn common_fence_aliases_resolve() {
    for token in ["jsx", "shell", "console", "toml"] {
        assert!(
            BlockHighlighter::for_language(token).is_some(),
            "expected highlighter for fence token {token}"
        );
    }
}

// Covers: TypeScript keywords and constants get role segments
// Owner: pure unit (syntax highlight)
#[test]
fn typescript_tokens_map_to_roles() {
    let mut highlighter = BlockHighlighter::for_language("ts").expect("bundled typescript syntax");
    let segments = highlighter.highlight_line("const answer: number = 42; // note");

    assert_eq!(
        segment_texts(&segments).concat(),
        "const answer: number = 42; // note"
    );
    assert_eq!(role_of(&segments, "const"), Some(SyntaxRole::Keyword));
    assert_eq!(role_of(&segments, "42"), Some(SyntaxRole::Constant));
    assert_eq!(role_of(&segments, "// note"), Some(SyntaxRole::Comment));
}

// Covers: empty line still yields one plain segment so layout keeps the row
// Owner: pure unit (syntax highlight)
#[test]
fn empty_line_yields_one_empty_plain_segment() {
    let mut highlighter = BlockHighlighter::for_language("rust").expect("bundled rust syntax");
    let segments = highlighter.highlight_line("");

    assert_eq!(segment_texts(&segments), vec![""]);
    assert_eq!(segments[0].role, None);
}

// Covers: write/edit and /diff resolve language from path, not fence tokens
// Owner: pure unit (path syntax lookup)
#[test]
fn path_extension_resolves_highlighter() {
    for path in ["src/lib.rs", "foo.ts", "pkg/main.py", "Makefile"] {
        assert!(
            BlockHighlighter::for_path(path).is_some(),
            "expected highlighter for path {path}"
        );
    }
    assert!(BlockHighlighter::for_path("/dev/null").is_none());
    assert!(BlockHighlighter::for_path("unknown.nope").is_none());
}

// Covers: callers map plain roles onto their own base style at paint time
// Owner: pure unit (highlight segment style)
#[test]
fn highlight_segment_style_uses_caller_plain() {
    let plain = Theme::text();
    let keyword = HighlightSegment {
        text: "let".into(),
        role: Some(SyntaxRole::Keyword),
    };
    let body = HighlightSegment {
        text: " x".into(),
        role: None,
    };
    assert_eq!(keyword.style(plain), Theme::syntax(SyntaxRole::Keyword));
    assert_eq!(body.style(plain), plain);
}

// Covers: match overlay splits segments on pattern ranges
// Owner: pure unit (search match overlay)
#[test]
fn match_overlay_splits_segments() {
    let segments = vec![
        HighlightSegment {
            text: "let ".into(),
            role: Some(SyntaxRole::Keyword),
        },
        HighlightSegment {
            text: "answer".into(),
            role: None,
        },
        HighlightSegment {
            text: " = 1".into(),
            role: None,
        },
    ];
    let ranges = match_byte_ranges(
        "let answer = 1",
        &MatchQuery::new(
            "answer", /*literal*/ false, /*case_sensitive*/ true,
        ),
    );
    assert_eq!(ranges, vec![(4, 10)]);
    let spans = spans_from_segments_with_matches(&segments, Theme::text(), &ranges);
    let answer = spans
        .iter()
        .find(|span| span.content.as_ref() == "answer")
        .expect("answer span");
    assert_eq!(answer.style, Theme::search_match(Theme::text()));
}

// Covers: literal match keeps regex metacharacters inert
// Owner: pure unit (search match semantics)
#[test]
fn literal_match_does_not_treat_dot_as_wildcard() {
    let ranges = match_byte_ranges(
        "a.b",
        &MatchQuery::new(".", /*literal*/ true, /*case_sensitive*/ true),
    );
    assert_eq!(ranges, vec![(1, 2)]);
}

// Covers: case-sensitive regex match skips differently-cased hits
// Owner: pure unit (search match semantics)
#[test]
fn case_sensitive_regex_skips_different_case() {
    let ranges = match_byte_ranges(
        "FOO foo",
        &MatchQuery::new("foo", /*literal*/ false, /*case_sensitive*/ true),
    );
    assert_eq!(ranges, vec![(4, 7)]);
}

// Covers: case-insensitive literal overlay matches Unicode case pairs like grep
// Owner: pure unit (search match semantics)
#[test]
fn case_insensitive_literal_matches_unicode_case_pair() {
    let ranges = match_byte_ranges(
        "prefix ä suffix",
        &MatchQuery::new("Ä", /*literal*/ true, /*case_sensitive*/ false),
    );
    assert_eq!(ranges, vec![(7, 9)], "ä is two UTF-8 bytes at offset 7");
}

// Covers: case-insensitive literal still treats metacharacters as plain text
// Owner: pure unit (search match semantics)
#[test]
fn case_insensitive_literal_keeps_metacharacters_inert() {
    let ranges = match_byte_ranges(
        "A.B a.b",
        &MatchQuery::new("a.b", /*literal*/ true, /*case_sensitive*/ false),
    );
    assert_eq!(ranges, vec![(0, 3), (4, 7)]);
}
