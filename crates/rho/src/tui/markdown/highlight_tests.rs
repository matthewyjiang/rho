use super::*;
use crate::tui::theme::{SyntaxRole, Theme};

fn segment_texts(segments: &[StyledSegment]) -> Vec<&str> {
    segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect()
}

fn style_of<'a>(segments: &'a [StyledSegment], text: &str) -> &'a Style {
    &segments
        .iter()
        .find(|segment| segment.text.trim() == text)
        .unwrap_or_else(|| panic!("no segment for {text:?} in {segments:?}"))
        .style
}

#[test]
fn rust_tokens_map_to_distinct_palette_roles() {
    let mut highlighter = BlockHighlighter::for_language("rust").expect("bundled rust syntax");
    let segments = highlighter.highlight_line("let answer = 42; // note");

    assert_eq!(
        segment_texts(&segments).concat(),
        "let answer = 42; // note"
    );
    assert_eq!(
        *style_of(&segments, "let"),
        Theme::markdown_syntax(SyntaxRole::Keyword)
    );
    assert_eq!(
        *style_of(&segments, "42"),
        Theme::markdown_syntax(SyntaxRole::Constant)
    );
    assert_eq!(
        *style_of(&segments, "// note"),
        Theme::markdown_syntax(SyntaxRole::Comment)
    );
}

#[test]
fn string_state_carries_across_lines() {
    let mut highlighter = BlockHighlighter::for_language("rust").expect("bundled rust syntax");
    highlighter.highlight_line("let text = \"open");
    let segments = highlighter.highlight_line("still inside");

    assert!(segments
        .iter()
        .all(|segment| segment.style == Theme::markdown_syntax(SyntaxRole::String)));
}

#[test]
fn unknown_language_has_no_highlighter() {
    assert!(BlockHighlighter::for_language("no-such-language").is_none());
}

// Covers: TypeScript fence tags must resolve after two-face syntax dump swap
// Owner: pure unit (markdown highlight language lookup)
#[test]
fn typescript_fence_tokens_resolve() {
    for token in ["ts", "tsx", "typescript"] {
        assert!(
            BlockHighlighter::for_language(token).is_some(),
            "expected highlighter for fence token {token}"
        );
    }
}

// Covers: common alias tags must map onto dump-native grammars
// Owner: pure unit (markdown highlight language lookup)
#[test]
fn common_fence_aliases_resolve() {
    for token in ["jsx", "shell", "console", "toml"] {
        assert!(
            BlockHighlighter::for_language(token).is_some(),
            "expected highlighter for fence token {token}"
        );
    }
}

// Covers: TypeScript keywords and types get role-colored segments
// Owner: pure unit (markdown highlight)
#[test]
fn typescript_tokens_map_to_palette_roles() {
    let mut highlighter = BlockHighlighter::for_language("ts").expect("bundled typescript syntax");
    let segments = highlighter.highlight_line("const answer: number = 42; // note");

    assert_eq!(
        segment_texts(&segments).concat(),
        "const answer: number = 42; // note"
    );
    assert_eq!(
        *style_of(&segments, "const"),
        Theme::markdown_syntax(SyntaxRole::Keyword)
    );
    assert_eq!(
        *style_of(&segments, "42"),
        Theme::markdown_syntax(SyntaxRole::Constant)
    );
    assert_eq!(
        *style_of(&segments, "// note"),
        Theme::markdown_syntax(SyntaxRole::Comment)
    );
}

#[test]
fn empty_line_yields_one_empty_base_segment() {
    let mut highlighter = BlockHighlighter::for_language("rust").expect("bundled rust syntax");
    let segments = highlighter.highlight_line("");

    assert_eq!(segment_texts(&segments), vec![""]);
    assert_eq!(segments[0].style, Theme::markdown_code_block());
}
