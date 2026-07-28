use super::*;
use pretty_assertions::assert_eq;
use ratatui::style::Style;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn display_width_ignores_control_characters_filtered_by_ratatui() {
    assert_eq!(display_width("left\tright"), 9);
    assert_eq!(display_width("left\rright"), 9);
    assert_eq!(display_width("left\u{1b}right"), 9);
}

// Covers: end truncation must keep its display-width and one-line output contract.
// Owner: pure TUI layout policy.
#[test]
fn truncate_one_line_matches_expected_outputs() {
    let cases = [
        ("zero width", "abc", 0, ""),
        ("width one", "abc", 1, "…"),
        ("empty", "", 4, ""),
        ("ASCII exact fit", "abc", 3, "abc"),
        ("ASCII truncation", "abcdef", 4, "abc…"),
        ("wide exact fit", "界a", 3, "界a"),
        ("wide truncation", "界ab", 3, "界…"),
        ("combining exact fit", "e\u{301}x", 2, "e\u{301}x"),
        ("combining truncation", "e\u{301}xy", 2, "e\u{301}…"),
        ("one newline", "ab\ncd", 5, "ab cd"),
        ("multiple newlines", "a\n\nbc", 4, "a  …"),
    ];

    for (name, text, width, expected) in cases {
        assert_eq!(truncate_one_line(text, width), expected, "{name}");
    }
}

// Covers: front truncation must keep its display-width and one-line output contract.
// Owner: pure TUI layout policy.
#[test]
fn truncate_keep_end_matches_expected_outputs() {
    let cases = [
        ("zero width", "abc", 0, ""),
        ("width one", "abc", 1, "…"),
        ("empty", "", 4, ""),
        ("ASCII exact fit", "abc", 3, "abc"),
        ("ASCII truncation", "abcdef", 4, "…def"),
        ("wide exact fit", "a界", 3, "a界"),
        ("wide truncation", "ab界", 3, "…界"),
        ("combining exact fit", "be\u{301}", 2, "be\u{301}"),
        ("combining truncation", "xabe\u{301}", 3, "…be\u{301}"),
        ("one newline", "ab\ncd", 5, "ab cd"),
        ("multiple newlines", "a\n\nbc", 4, "… bc"),
    ];

    for (name, text, width, expected) in cases {
        assert_eq!(truncate_keep_end(text, width), expected, "{name}");
    }
}

#[test]
fn complete_visual_prefix_preserves_trailing_newline_state() {
    assert_eq!(complete_visual_prefix_byte_index("a\n", 10), "a\n".len());
    assert_eq!(
        complete_visual_prefix_byte_index("a\n\n", 10),
        "a\n\n".len()
    );
    assert_eq!(complete_visual_prefix_byte_index("a\nb", 10), "a\n".len());
}

#[test]
fn complete_visual_prefix_keeps_multibyte_boundaries() {
    assert_eq!(complete_visual_prefix_byte_index("éa", 2), "éa".len());
    assert_eq!(complete_visual_prefix_byte_index("éab", 2), "éa".len());
}

#[test]
fn complete_visual_prefix_wraps_at_exact_width() {
    assert_eq!(complete_visual_prefix_byte_index("abc", 3), 3);
    assert_eq!(complete_visual_prefix_byte_index("abcd", 3), 3);
    assert_eq!(complete_visual_prefix_byte_index("abcdef", 3), 6);
}

#[test]
fn wrapped_text_prefers_whitespace_boundaries() {
    let mut lines = Vec::new();
    push_wrapped_text(
        &mut lines,
        "hello wide world",
        10,
        Style::default(),
        LineFill::Natural,
    );

    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(
        rendered,
        vec!["hello wide".to_string(), " world".to_string()]
    );
}

#[test]
fn complete_visual_prefix_prefers_whitespace_boundaries() {
    assert_eq!(
        complete_visual_prefix_byte_index("hello wide", 8),
        "hello ".len()
    );
    assert_eq!(
        complete_visual_prefix_byte_index("hello wide", 10),
        "hello wide".len()
    );
}

#[test]
fn wrapped_text_preserves_leading_repeated_and_trailing_whitespace() {
    let mut lines = Vec::new();
    push_wrapped_text(
        &mut lines,
        "  indented\na  b\ntrail  ",
        20,
        Style::default(),
        LineFill::Natural,
    );

    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(
        rendered,
        vec![
            "  indented".to_string(),
            "a  b".to_string(),
            "trail  ".to_string()
        ]
    );
}

#[test]
fn wrapped_text_preserves_tabs_and_whitespace_only_lines() {
    let mut lines = Vec::new();
    push_wrapped_text(
        &mut lines,
        "\tindented\n   ",
        20,
        Style::default(),
        LineFill::Natural,
    );

    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(rendered, vec!["\tindented".to_string(), "   ".to_string()]);
}

#[test]
fn wrapped_text_preserves_whitespace_when_breaking_at_boundary() {
    let mut lines = Vec::new();
    push_wrapped_text(
        &mut lines,
        "hello   wide",
        8,
        Style::default(),
        LineFill::Natural,
    );

    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(rendered, vec!["hello   ".to_string(), "wide".to_string()]);
}

#[test]
fn complete_visual_prefix_and_rendering_agree_on_whitespace_boundary() {
    let text = "hello   wide";
    let split = complete_visual_prefix_byte_index(text, 8);
    let mut lines = Vec::new();
    push_wrapped_text(&mut lines, text, 8, Style::default(), LineFill::Natural);

    assert_eq!(&text[..split], "hello   ");
    assert_eq!(line_text(&lines[0]), "hello   ");
}

#[test]
fn complete_visual_prefix_and_rendering_agree_on_exact_width_trailing_space() {
    let text = "abc ";
    let split = complete_visual_prefix_byte_index(text, 3);
    let mut lines = Vec::new();
    push_wrapped_text(&mut lines, text, 3, Style::default(), LineFill::Natural);

    assert_eq!(&text[..split], "abc");
    assert_eq!(
        lines.iter().map(line_text).collect::<Vec<_>>(),
        vec!["abc".to_string(), " ".to_string()]
    );
}

#[test]
fn wrapped_text_handles_wide_chars_in_narrow_width() {
    let mut lines = Vec::new();
    push_wrapped_text(&mut lines, "你a", 1, Style::default(), LineFill::Natural);

    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(rendered, vec!["你".to_string(), "a".to_string()]);
}

#[test]
fn wrapped_text_keeps_list_syntax_out_of_generic_wrapping() {
    assert_eq!(
        wrap_line_at_whitespace(
            "- fixtures/downstream/no-default-features/Cargo.toml: package 0.0.0",
            39,
        ),
        vec![
            "- ".to_string(),
            "fixtures/downstream/no-default-features".to_string(),
            "/Cargo.toml: package 0.0.0".to_string(),
        ]
    );
}

#[test]
fn long_words_still_hard_wrap() {
    let mut lines = Vec::new();
    push_wrapped_text(
        &mut lines,
        "abcdefghijk",
        5,
        Style::default(),
        LineFill::Natural,
    );

    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(
        rendered,
        vec!["abcde".to_string(), "fghij".to_string(), "k".to_string()]
    );
}

#[test]
fn stream_fragment_rendering_preserves_blank_lines() {
    let mut lines = Vec::new();
    push_wrapped_text(&mut lines, "a\n\n", 10, Style::default(), LineFill::Natural);

    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(rendered, vec!["a".to_string(), String::new()]);
}

#[test]
fn visual_cursor_movement_clamps_to_shorter_explicit_line() {
    let input = "ab\ncdef";
    let lines = input_visual_lines(input, 80);

    assert_eq!(input_cursor_index_on_visual_line(input, &lines, 0, 4), 2);
}

#[test]
fn visual_cursor_movement_uses_wide_character_columns() {
    let input = "界a界b";
    let lines = input_visual_lines(input, 3);

    assert_eq!(lines, vec!["界a", "界b"]);
    assert_eq!(input_cursor_index_on_visual_line(input, &lines, 0, 2), 1);
}

#[test]
fn visual_cursor_movement_preserves_ascii_wrapped_column() {
    let input = "abcdef";
    let lines = input_visual_lines(input, 4);

    assert_eq!(input_cursor_index_on_visual_line(input, &lines, 0, 2), 2);
}
