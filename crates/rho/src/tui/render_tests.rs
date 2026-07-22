use super::*;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Paragraph, Widget},
};

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn line_styles(line: &Line<'_>) -> Vec<Style> {
    line.spans.iter().map(|span| span.style).collect()
}

#[test]
fn reasoning_entry_renders_inline_markdown_and_remains_dim() {
    let rendered = render_entry(
        &Entry::Reasoning("thinking with **bold** and *emphasis*".into()),
        80,
        10,
    );
    let content_line = rendered
        .lines
        .iter()
        .find(|line| line_text(line).contains("thinking with"))
        .expect("reasoning content line");

    assert_eq!(
        line_text(content_line).trim(),
        "thinking with bold and emphasis"
    );
    let bold = content_line
        .spans
        .iter()
        .find(|span| span.content == "bold")
        .expect("bold span");
    let italic = content_line
        .spans
        .iter()
        .find(|span| span.content == "emphasis")
        .expect("italic span");
    assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    assert!(bold.style.add_modifier.contains(Modifier::DIM));
    assert!(italic.style.add_modifier.contains(Modifier::ITALIC));
    assert!(italic.style.add_modifier.contains(Modifier::DIM));
}

#[test]
fn reasoning_entry_renders_thought_duration_footer() {
    let rendered = render_entry(
        &Entry::Reasoning(super::super::ReasoningEntry {
            text: "because reasons".into(),
            thought_for: Some(std::time::Duration::from_millis(3_200)),
        }),
        80,
        10,
    );
    let footer = rendered
        .lines
        .iter()
        .find(|line| line_text(line).contains("Thought for 3.2s"))
        .expect("thought footer");
    assert!(footer.spans.iter().any(|span| {
        span.style.add_modifier.contains(Modifier::DIM) && span.content.contains("Thought for 3.2s")
    }));

    let summary_only = render_entry(
        &Entry::Reasoning(super::super::ReasoningEntry::summary_only(
            std::time::Duration::from_secs(65),
        )),
        80,
        10,
    );
    assert!(summary_only
        .lines
        .iter()
        .any(|line| line_text(line).contains("Thought for 1m 5s")));
}

#[test]
fn display_width_ignores_control_characters_filtered_by_ratatui() {
    assert_eq!(display_width("left\tright"), 9);
    assert_eq!(display_width("left\rright"), 9);
    assert_eq!(display_width("left\u{1b}right"), 9);
}

#[test]
fn tool_block_with_tabs_fills_the_full_width() {
    let width = 20;
    let mut lines = Vec::new();
    push_tool_block_with_style(
        &mut lines,
        &["bash".into(), "one\ttwo\tthree".into()],
        width,
        10,
        false,
        Style::default().bg(Color::Green),
        false,
    );

    assert!(lines
        .iter()
        .all(|line| display_width(&line_text(line)) == width));

    let area = Rect::new(0, 0, width as u16, lines.len() as u16);
    let mut buffer = Buffer::empty(area);
    Paragraph::new(lines).render(area, &mut buffer);
    assert!((0..area.height)
        .all(|row| { (0..area.width).all(|column| buffer[(column, row)].bg == Color::Green) }));
}

#[test]
fn narrow_picker_rows_do_not_exceed_width() {
    let picker = UiPicker::new(
        "models",
        "enter confirm",
        vec![PickerItem {
            label: "very-wide-model-name".into(),
            detail: Some("very wide detail".into()),
            preview: Some("wide preview".into()),
            badge: Some(crate::tui::PickerBadge {
                text: "selected".into(),
                tone: PickerBadgeTone::Selected,
            }),
            value: "very-wide-model-name".into(),
        }],
        crate::tui::PickerAction::SelectModel,
    );

    let lines = picker_lines(&picker, 4);

    assert!(
        lines
            .iter()
            .all(|line| display_width(&line_text(line)) <= 4),
        "{:#?}",
        lines.iter().map(line_text).collect::<Vec<_>>()
    );
}

#[test]
fn list_picker_height_stays_stable_when_selected_detail_is_missing() {
    let mut picker = UiPicker::new(
        "models",
        "enter confirm",
        vec![
            PickerItem {
                label: "plain".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "plain".into(),
            },
            PickerItem {
                label: "detailed".into(),
                detail: Some("extra context".into()),
                preview: None,
                badge: None,
                value: "detailed".into(),
            },
        ],
        crate::tui::PickerAction::SelectModel,
    );

    let first_height = picker_lines(&picker, 80).len();
    picker.select_next();

    assert_eq!(picker_lines(&picker, 80).len(), first_height);
}

#[test]
fn assistant_markdown_styles_inline_code_bold_and_italic() {
    let lines = entry_lines(
        &Entry::Assistant("use `cargo test`, then **ship** the *fix*".into()),
        80,
        10,
    );

    let content = &lines[1];
    assert_eq!(line_text(content), " use cargo test, then ship the fix ");
    let styles = line_styles(content);
    assert!(styles.contains(&Theme::markdown_inline_code()));
    assert!(styles.contains(&Theme::markdown_bold()));
    assert!(styles.contains(&Theme::markdown_italic()));
    assert_eq!(Theme::markdown_bold().fg, None);
    assert_eq!(Theme::markdown_italic().fg, None);
}

#[test]
fn assistant_markdown_styles_code_blocks() {
    let lines = entry_lines(&Entry::Assistant("```rust\nlet x = 1;\n```".into()), 80, 10);

    assert!(line_text(&lines[1]).contains("╭"));
    assert!(line_text(&lines[2]).contains("│ let x = 1;"));
    assert!(line_text(&lines[3]).contains("╰"));
    assert_eq!(lines[2].spans[1].style, Theme::markdown_code_block());
}

#[test]
fn assistant_markdown_renders_divider_lines() {
    let lines = entry_lines(&Entry::Assistant("before\n---\nafter".into()), 20, 10);

    assert_eq!(line_text(&lines[1]), " before ");
    assert_eq!(line_text(&lines[2]), format!(" {} ", "─".repeat(18)));
    assert_eq!(lines[2].spans[1].style, Theme::dim());
    assert_eq!(line_text(&lines[3]), " after ");
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
