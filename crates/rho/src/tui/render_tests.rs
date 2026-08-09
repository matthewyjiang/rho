use super::*;
use crate::tui::{PickerAction, PickerItem, UiPicker};
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

// Covers: soft wrap must not put the break space at the start of the next line.
// Owner: pure unit (wrap layout math)
#[test]
fn wrapped_text_prefers_whitespace_boundaries() {
    let cases = [
        ("hello wide world", 10, vec!["hello wide", "world"]),
        // Overflow lands on the break space itself.
        ("models.dev mapping.", 10, vec!["models.dev", "mapping."]),
        // Extra spaces after a width-split must not indent the continuation.
        ("hello  world", 6, vec!["hello ", "world"]),
    ];

    for (text, width, expected) in cases {
        let mut lines = Vec::new();
        push_wrapped_text(&mut lines, text, width, Style::default(), LineFill::Natural);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(
            rendered,
            expected
                .iter()
                .map(|part| (*part).to_string())
                .collect::<Vec<_>>(),
            "text {text:?} width {width}"
        );
    }
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

// Covers: a trailing wrap-boundary space is drained for streaming but not shown
// as its own indented continuation line.
// Owner: pure unit (wrap layout math)
#[test]
fn complete_visual_prefix_and_rendering_agree_on_exact_width_trailing_space() {
    let text = "abc ";
    let split = complete_visual_prefix_byte_index(text, 3);
    let mut lines = Vec::new();
    push_wrapped_text(&mut lines, text, 3, Style::default(), LineFill::Natural);

    assert_eq!(&text[..split], "abc");
    assert_eq!(
        lines.iter().map(line_text).collect::<Vec<_>>(),
        vec!["abc".to_string()]
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

// Covers: up/down arrow must land on the character under the target column,
// including on rows past the first, where hard newlines are skipped but soft
// wraps are not.
// Owner: pure unit (composer layout math)
#[test]
fn visual_cursor_index_maps_row_and_column_to_char_index() {
    let cases = [
        // (input, width, row, column, expected char index)
        ("ab\ncdef", 80, 0, 4, 2),
        ("界a界b", 3, 0, 2, 1),
        ("abcdef", 4, 0, 2, 2),
        // Row 1 after a hard newline: the '\n' itself is not addressable.
        ("ab\ncdef", 80, 1, 2, 5),
        ("ab\ncdef", 80, 1, 9, 7),
        // Row 1 after a soft wrap: every character stays addressable.
        ("abcdef", 4, 1, 1, 5),
        ("界a界b", 3, 1, 2, 3),
        // Two hard newlines put row 2 past both of them.
        ("a\nb\ncd", 80, 2, 1, 5),
        // Word wrap: break after the space, second row starts at "world".
        ("hello world", 8, 1, 0, 6),
        ("hello world", 8, 1, 2, 8),
        // Exact-width first word: preserve mode keeps the break space on row 1.
        ("hello world", 5, 1, 0, 5),
        ("hello world", 5, 1, 1, 6),
    ];

    for (input, width, row, column, expected) in cases {
        let lines = input_visual_lines(input, width);
        assert_eq!(
            input_cursor_index_on_visual_line(input, &lines, row, column),
            expected,
            "input {input:?} width {width} row {row} column {column} lines {lines:?}"
        );
    }
}

// Covers: click/hit-test mapping uses the editable visual lines (including the
// trailing empty row after a full-width wrap) so caret placement matches paint.
// Owner: pure unit (composer layout math)
#[test]
fn input_char_index_at_position_matches_editable_layout() {
    assert_eq!(input_char_index_at_position("hello", 80, 0, 2), 2);
    assert_eq!(input_char_index_at_position("hello", 80, 0, 99), 5);
    // Soft-wrapped second row.
    assert_eq!(input_char_index_at_position("abcdefghij", 5, 1, 2), 7);
    // Full-width first visual line leaves an empty editable row for the caret.
    assert_eq!(input_char_index_at_position("abcde", 5, 1, 0), 5);
}

// Covers: composer soft-wraps on word boundaries instead of mid-word hard cuts.
// Owner: pure unit (composer layout math)
#[test]
fn composer_input_word_wraps_at_whitespace() {
    assert_eq!(
        input_visual_lines("hello wide world", 10),
        vec!["hello wide".to_string(), " world".to_string()]
    );
    // Long tokens still hard-wrap when no break fits.
    assert_eq!(
        input_visual_lines("abcdefghijk", 5),
        vec!["abcde".to_string(), "fghij".to_string(), "k".to_string()]
    );
    // Hard newlines still split first; soft wrap applies per logical line.
    assert_eq!(
        input_visual_lines("hello world\nnext line here", 10),
        vec![
            "hello ".to_string(),
            "world".to_string(),
            "next line ".to_string(),
            "here".to_string(),
        ]
    );
}

// Covers: highlight lockstep stays aligned across preserved soft-wrap spaces.
// Owner: pure unit (composer layout math)
#[test]
fn composer_input_lines_highlight_survives_word_wrap() {
    let lines = input_lines("hello world", 8, Some(6..11));
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(rendered, vec!["hello ".to_string(), "world".to_string()]);
    assert_eq!(lines[1].spans.len(), 1);
    assert_eq!(lines[1].spans[0].content.as_ref(), "world");
    assert!(lines[1].spans[0]
        .style
        .add_modifier
        .contains(ratatui::style::Modifier::REVERSED));
}

// Covers: a picker must list more entries on a tall terminal instead of staying
// at the count that fits the default-height fallback.
// Owner: pure unit (picker line generation); a PTY scenario would re-assert this
// same line math through a slower path.
#[test]
fn picker_lists_more_items_on_a_taller_viewport() {
    let items = (0..40)
        .map(|index| PickerItem {
            section: None,
            label: format!("model-{index}"),
            detail: None,
            preview: None,
            badge: None,
            value: format!("model-{index}"),
            selection_verb: None,
        })
        .collect();
    let picker = UiPicker::new("models", items, PickerAction::SelectModel);

    let item_rows = |height: usize| {
        picker_lines(&picker, 80, height)
            .iter()
            // Item rows carry a selection marker before the label.
            .filter(|line| line_text(line).contains("model-"))
            .count()
    };

    assert_eq!(item_rows(18), 8);
    assert_eq!(item_rows(40), 30);
    // A viewport with no room left still shows the selected item.
    assert_eq!(item_rows(4), 1);
}
