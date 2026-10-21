use super::*;
use crate::tui::theme::{SyntaxRole, Theme};
use ratatui::style::{Color, Style};

fn highlighter(token: &str) -> BlockHighlighter {
    warm_syntax_set();
    BlockHighlighter::for_language(token).expect("bundled syntax")
}

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

// Covers: rust and TypeScript fence tokens map onto distinct syntax roles
// Owner: pure unit (syntax highlight)
#[test]
fn rust_tokens_map_to_distinct_roles() {
    for (language, line, keyword) in [
        ("rust", "let answer = 42; // note", "let"),
        ("ts", "const answer: number = 42; // note", "const"),
    ] {
        let mut highlighter = highlighter(language);
        let segments = highlighter.highlight_line(line);

        assert_eq!(segment_texts(&segments).concat(), line, "{language}");
        assert_eq!(
            role_of(&segments, keyword),
            Some(SyntaxRole::Keyword),
            "{language}"
        );
        assert_eq!(
            role_of(&segments, "42"),
            Some(SyntaxRole::Constant),
            "{language}"
        );
        assert_eq!(
            role_of(&segments, "// note"),
            Some(SyntaxRole::Comment),
            "{language}"
        );
    }
}

// Covers: multi-line string grammar state survives across highlight_line calls
// Owner: pure unit (syntax highlight)
#[test]
fn string_state_carries_across_lines() {
    let mut highlighter = highlighter("rust");
    highlighter.highlight_line("let text = \"open");
    let segments = highlighter.highlight_line("still inside");

    assert!(segments
        .iter()
        .all(|segment| segment.role == Some(SyntaxRole::String)));
}

// Covers: TypeScript fence tags and common aliases resolve to dump-native
// grammars; unknown languages fall back to no highlighter
// Owner: pure unit (syntax language lookup)
#[test]
fn fence_tokens_and_aliases_resolve() {
    warm_syntax_set();
    for token in [
        "ts",
        "tsx",
        "typescript",
        "jsx",
        "shell",
        "console",
        "toml",
        "ps1",
        "powershell",
    ] {
        assert!(
            BlockHighlighter::for_language(token).is_some(),
            "expected highlighter for fence token {token}"
        );
    }
    assert!(BlockHighlighter::for_language("no-such-language").is_none());
}

// Covers: empty line still yields one plain segment so layout keeps the row
// Owner: pure unit (syntax highlight)
#[test]
fn empty_line_yields_one_empty_plain_segment() {
    let mut highlighter = highlighter("rust");
    let segments = highlighter.highlight_line("");

    assert_eq!(segment_texts(&segments), vec![""]);
    assert_eq!(segments[0].role, None);
}

// Covers: write/edit and /diff resolve language from path, not fence tokens
// Owner: pure unit (path syntax lookup)
#[test]
fn path_extension_resolves_highlighter() {
    warm_syntax_set();
    for path in [
        "src/lib.rs",
        "foo.ts",
        "pkg/main.py",
        "Makefile",
        "script.ps1",
    ] {
        assert!(
            BlockHighlighter::for_path(path).is_some(),
            "expected highlighter for path {path}"
        );
    }
    assert!(BlockHighlighter::for_path("/dev/null").is_none());
    assert!(BlockHighlighter::for_path("unknown.nope").is_none());
}

// Covers: PowerShell cmdlets highlight, double-dash flags do not
// Owner: pure unit (bundled PowerShell grammar)
#[test]
fn powershell_cmdlets_do_not_match_double_dash_flags() {
    let mut highlighter = highlighter("powershell");
    let segments = highlighter.highlight_line("git commit --no-verify; Write-Output 1");
    assert_eq!(
        role_of(&segments, "Write-Output"),
        Some(SyntaxRole::Function)
    );
    assert!(
        segments
            .iter()
            .filter(|segment| segment.text.contains("no-verify"))
            .all(|segment| segment.role != Some(SyntaxRole::Function)),
        "double-dash flags must not paint as cmdlets: {segments:?}"
    );
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
    let washed = Style::default().fg(Color::White).bg(Color::Rgb(12, 40, 24));
    assert_eq!(keyword.style(washed).bg, washed.bg);
    assert_eq!(
        keyword.style(washed).fg,
        Theme::syntax(SyntaxRole::Keyword).fg
    );
    assert_eq!(body.style(washed), washed);
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

// Covers: literal queries keep regex metacharacters inert, case-sensitive
// queries skip other cases, and case-insensitive literals fold Unicode case
// pairs like grep
// Owner: pure unit (search match semantics)
#[test]
fn literal_match_does_not_treat_dot_as_wildcard() {
    for (case, haystack, pattern, literal, case_sensitive, expected) in [
        ("literal dot", "a.b", ".", true, true, vec![(1, 2)]),
        ("cased regex", "FOO foo", "foo", false, true, vec![(4, 7)]),
        // ä is two UTF-8 bytes at offset 7.
        (
            "unicode case pair",
            "prefix ä suffix",
            "Ä",
            true,
            false,
            vec![(7, 9)],
        ),
        (
            "caseless literal metacharacters",
            "A.B a.b",
            "a.b",
            true,
            false,
            vec![(0, 3), (4, 7)],
        ),
    ] {
        let query = MatchQuery::new(pattern, literal, case_sensitive);
        assert_eq!(match_byte_ranges(haystack, &query), expected, "{case}");
    }
}
