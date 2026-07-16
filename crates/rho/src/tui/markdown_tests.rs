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
    let mut code_fence = CodeFenceState::default();
    push_wrapped_markdown_without_copy_button_from_fence_state(
        &mut lines,
        "```rust\nlet x = 1;",
        40,
        &mut code_fence,
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

#[test]
fn renders_a_realistic_agent_workflow_diagram() {
    let source = "flowchart TD\n    A[User submits a request] --> B[Agent analyzes the task]\n    B --> C{Is more information needed?}\n    C -->|Yes| D[Ask a clarifying question]\n    D --> A\n    C -->|No| E[Inspect relevant files]\n    E --> F[Plan the change]\n    F --> G[Edit the code]\n    G --> H[Run formatting and tests]\n    H --> I{Did validation pass?}\n    I -->|No| J[Diagnose and fix failures]\n    J --> H\n    I -->|Yes| K[Summarize the result]";
    let mut in_code_block = false;
    let rendered = render_markdown(
        &format!("```mermaid\n{source}\n```"),
        100,
        &mut in_code_block,
    );
    let text = rendered.lines.iter().map(line_text).collect::<Vec<_>>();

    assert!(text[0].contains("MERMAID"));
    assert!(text.iter().any(|line| line.contains("User submits")));
    assert!(text.iter().any(|line| line.contains("Summarize")));
    assert!(!text.iter().any(|line| line.contains("flowchart TD")));
    assert!(text.iter().all(|line| display_width(line) <= 100));
    assert_eq!(rendered.code_blocks[0].text, source);
}

#[test]
fn closed_mermaid_fence_renders_a_titled_diagram_and_preserves_source() {
    let source = "flowchart LR\n    A[Parse] --> B[Render]";
    let markdown = format!("before\n```MeRmAiD theme=dark\n{source}\n```\nafter");
    let mut in_code_block = false;
    let rendered = render_markdown(&markdown, 80, &mut in_code_block);
    let text = rendered.lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(text.first().map(String::as_str), Some("before"));
    assert!(text[1].starts_with("╭─ MERMAID "), "{}", text[1]);
    assert!(text.iter().any(|line| line.contains("Parse")));
    assert!(text.iter().any(|line| line.contains("Render")));
    assert_eq!(text.last().map(String::as_str), Some("after"));
    assert!(!text.iter().any(|line| line.contains("flowchart LR")));
    assert_eq!(rendered.code_blocks.len(), 1);
    assert_eq!(rendered.code_blocks[0].top_line, 1);
    assert_eq!(rendered.code_blocks[0].text, source);
    assert_eq!(rendered.code_blocks[0].copy_columns, 73..79);
    assert!(rendered.lines[1]
        .spans
        .iter()
        .any(|span| span.content.as_ref() == " COPY "));
}

#[test]
fn mermaid_canvas_is_tightly_cropped_and_uniformly_centered_in_full_width_panel() {
    let mut in_code_block = false;
    let rendered = render_markdown(
        "```mermaid\nflowchart TD\nA[Tea] --> B{Milk?}\nB --> C[Drink]\n```",
        80,
        &mut in_code_block,
    );
    let rows = rendered.lines.iter().map(line_text).collect::<Vec<_>>();

    assert!(rows.iter().all(|row| display_width(row) == 80));
    let content = &rows[1..rows.len() - 1];
    let occupied = content
        .iter()
        .flat_map(|row| {
            row.chars()
                .enumerate()
                .filter(|(_, character)| !character.is_whitespace() && *character != '│')
                .map(|(column, _)| column)
        })
        .collect::<Vec<_>>();
    let left = *occupied.iter().min().unwrap();
    let right = *occupied.iter().max().unwrap();
    let left_margin = left.saturating_sub(2);
    let right_margin = 77usize.saturating_sub(right);
    assert!(
        left_margin.abs_diff(right_margin) <= 1,
        "{}",
        rows.join("\n")
    );
    assert!(content.iter().any(|row| row
        .chars()
        .nth(left)
        .is_some_and(|character| character != ' ')));
    assert!(content.iter().any(|row| row
        .chars()
        .nth(right)
        .is_some_and(|character| character != ' ')));
}

#[test]
fn code_fence_closers_match_marker_length_and_allow_only_whitespace() {
    let opening = parse_opening_fence("   ````mermaid").expect("valid opening fence");
    assert_eq!(opening.marker, '`');
    assert_eq!(opening.length, 4);
    assert!(!is_closing_fence("```", opening));
    assert!(!is_closing_fence("~~~~", opening));
    assert!(!is_closing_fence("````not-a-close", opening));
    assert!(is_closing_fence("  `````   ", opening));
    assert!(parse_opening_fence("    ```rust").is_none());
    assert!(parse_opening_fence("```rust`edition").is_none());
}

#[test]
fn streamed_code_fence_state_preserves_marker_and_length_across_chunks() {
    let mut state = CodeFenceState::default();
    update_code_block_state("````mermaid\nflowchart TD", &mut state);
    assert!(state.is_open());
    update_code_block_state("```", &mut state);
    assert!(state.is_open());
    update_code_block_state("````", &mut state);
    assert!(!state.is_open());

    update_code_block_state("~~~~mermaid", &mut state);
    assert!(state.is_open());
    update_code_block_state("```", &mut state);
    assert!(state.is_open());
    update_code_block_state("~~~~", &mut state);
    assert!(!state.is_open());
}

#[test]
fn mermaid_scanner_keeps_an_invalid_closer_inside_the_raw_block() {
    let mut in_code_block = false;
    let rendered = render_markdown(
        "````mermaid\nflowchart TD\nA[one]\n```not-a-close\nA --> B[two]\n````",
        80,
        &mut in_code_block,
    );
    let text = rendered
        .lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("MERMAID"), "{text}");
    assert!(text.contains("one"), "{text}");
    assert!(text.contains("two"), "{text}");
}

#[test]
fn open_mermaid_fence_stays_raw_until_closed() {
    let mut in_code_block = false;
    let open = render_markdown("```mermaid\nflowchart LR\nA --> B", 60, &mut in_code_block);
    let open_text = open.lines.iter().map(line_text).collect::<Vec<_>>();

    assert!(in_code_block);
    assert!(open_text.iter().any(|line| line.contains("flowchart LR")));
    assert!(!open_text.iter().any(|line| line.contains("MERMAID")));

    let mut in_code_block = false;
    let closed = render_markdown(
        "```mermaid\nflowchart LR\nA --> B\n```",
        60,
        &mut in_code_block,
    );
    assert!(!in_code_block);
    assert!(line_text(&closed.lines[0]).contains("MERMAID"));
    assert!(!closed
        .lines
        .iter()
        .map(line_text)
        .any(|line| line.contains("flowchart LR")));
}

#[test]
fn malformed_and_too_wide_mermaid_fences_use_normal_code_blocks() {
    for (source, width) in [
        ("not-a-diagram", 60),
        ("flowchart LR\nA[a label that is much too wide]", 8),
    ] {
        let mut in_code_block = false;
        let markdown = format!("```mermaid\n{source}\n```");
        let rendered = render_markdown(&markdown, width, &mut in_code_block);
        let text = rendered.lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(!in_code_block);
        assert!(!text[0].contains("MERMAID"));
        assert!(text
            .iter()
            .any(|line| line.contains("flow") || line.contains("not-")));
        assert_eq!(rendered.code_blocks[0].text, source);
    }
}

#[test]
fn mermaid_render_reflows_to_the_requested_transcript_width() {
    let markdown = "```mermaid\nflowchart LR\nA[Parse] --> B[Render]\n```";
    let mut wide_state = false;
    let wide = markdown_lines(markdown, 80, &mut wide_state);
    let mut narrow_state = false;
    let narrow = markdown_lines(markdown, 36, &mut narrow_state);

    assert!(wide
        .iter()
        .all(|line| display_width(&line_text(line)) <= 80));
    assert!(narrow
        .iter()
        .all(|line| display_width(&line_text(line)) <= 36));
    assert_ne!(
        wide.iter().map(line_text).collect::<Vec<_>>(),
        narrow.iter().map(line_text).collect::<Vec<_>>()
    );
}
