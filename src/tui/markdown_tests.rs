use super::*;
use ratatui::style::{Modifier, Style};

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
fn styles_inline_code_bold_italic_and_links_without_markers() {
    let mut in_code_block = false;
    let lines = markdown_lines(
        "use `cargo test`, then **ship** the *fix*, [docs](https://example.com), and https://example.com",
        120,
        &mut in_code_block,
    );

    assert_eq!(
        line_text(&lines[0]),
        "use cargo test, then ship the fix, docs: https://example.com, and https://example.com"
    );
    let styles = line_styles(&lines[0]);
    assert!(styles.contains(&Theme::markdown_inline_code()));
    assert!(styles.contains(&Theme::markdown_bold()));
    assert!(styles.contains(&Theme::markdown_italic()));
    assert!(styles.contains(&Theme::markdown_link()));
    assert_eq!(Theme::markdown_bold().fg, None);
    assert_eq!(Theme::markdown_italic().fg, None);
    assert_eq!(Theme::markdown_link().fg, Theme::accent().fg);
    assert!(Theme::markdown_link().has_modifier(Modifier::UNDERLINED));
    assert_eq!(
        styles
            .iter()
            .filter(|style| **style == Theme::markdown_link())
            .count(),
        2
    );
}

#[test]
fn preserves_underscores_inside_identifiers() {
    let mut in_code_block = false;
    let lines = markdown_lines(
        "keep foo_bar_baz literal but style _this_",
        120,
        &mut in_code_block,
    );

    assert_eq!(
        line_text(&lines[0]),
        "keep foo_bar_baz literal but style this"
    );
    assert!(line_styles(&lines[0]).contains(&Theme::markdown_italic()));
}

#[test]
fn wraps_long_unicode_styled_lines_without_losing_text_or_styles() {
    let plain_prefix = "éλ".repeat(256);
    let bold = "你🙂".repeat(256);
    let plain_suffix = "界ß".repeat(256);
    let markdown = format!("{plain_prefix} **{bold}** {plain_suffix}");
    let expected = format!("{plain_prefix} {bold} {plain_suffix}");
    let mut in_code_block = false;

    let lines = markdown_lines(&markdown, 17, &mut in_code_block);
    let rendered = lines.iter().map(line_text).collect::<String>();
    let rendered_bold = lines
        .iter()
        .flat_map(|line| &line.spans)
        .filter(|span| span.style == Theme::markdown_bold())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(rendered, expected);
    assert_eq!(rendered_bold, bold);
    assert!(lines
        .iter()
        .all(|line| display_width(&line_text(line)) <= 17));
}

#[test]
fn stream_preview_renderer_can_hide_inactive_copy_buttons() {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    push_wrapped_markdown_without_copy_button(
        &mut lines,
        "```rust\nlet x = 1;",
        40,
        &mut in_code_block,
    );
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(rendered.contains("let x = 1;"), "{rendered}");
    assert!(!rendered.contains("COPY"), "{rendered}");
}

#[test]
fn renders_code_blocks_with_closed_borders() {
    let mut in_code_block = false;
    let lines = markdown_lines("```rust\nlet x = 1;\n```", 20, &mut in_code_block);

    assert_eq!(line_text(&lines[0]), "╭──────────── COPY ╮");
    assert_eq!(line_text(&lines[1]), "│ let x = 1;       │");
    assert_eq!(line_text(&lines[2]), "╰──────────────────╯");
    assert_eq!(lines[0].spans[1].content.as_ref(), " COPY ");
    assert_eq!(
        lines[0].spans[1].style,
        Theme::markdown_code_copy_button(/*hovered*/ false)
    );
    assert_eq!(lines[1].spans[0].style, Theme::markdown_code_block());
}

#[test]
fn code_block_padding_uses_display_width() {
    let mut in_code_block = false;
    let lines = markdown_lines("```\n你\n```", 6, &mut in_code_block);

    assert_eq!(line_text(&lines[1]), "│ 你 │");
    assert_eq!(display_width(&line_text(&lines[1])), 6);
}

#[test]
fn code_blocks_preserve_markdown_markers_as_literal_text() {
    let mut in_code_block = false;
    let lines = markdown_lines(
        "```rust\nfn __init__() { println!(\"*ok*\"); }\n```",
        80,
        &mut in_code_block,
    );

    assert!(line_text(&lines[1]).contains("fn __init__() { println!(\"*ok*\"); }"));
    assert_eq!(line_styles(&lines[1]), vec![Theme::markdown_code_block()]);
}

#[test]
fn renders_divider_lines() {
    let mut in_code_block = false;
    let lines = markdown_lines("before\n---\nafter", 20, &mut in_code_block);

    assert_eq!(line_text(&lines[0]), "before");
    assert_eq!(line_text(&lines[1]), "─".repeat(20));
    assert_eq!(lines[1].spans[0].style, Theme::dim());
    assert_eq!(line_text(&lines[2]), "after");
}
