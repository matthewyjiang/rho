use super::*;
use ratatui::style::Style;

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
fn keeps_list_markers_with_a_long_path_in_narrow_output() {
    for marker in ["-", "1.", "2)"] {
        let markdown =
            format!("{marker} fixtures/downstream/no-default-features/Cargo.toml: package 0.0.0");
        let mut in_code_block = false;
        let lines = markdown_lines(&markdown, 39, &mut in_code_block);

        let first_line_suffix_len = 39 - marker.len() - 1;
        let path = "fixtures/downstream/no-default-features/Cargo.toml: package 0.0.0";
        assert_eq!(
            lines.iter().map(line_text).collect::<Vec<_>>(),
            vec![
                format!("{marker} {}", &path[..first_line_suffix_len]),
                path[first_line_suffix_len..].to_string(),
            ]
        );
    }
}

#[test]
fn streams_list_lines_at_the_same_wrap_boundary_as_final_rendering() {
    let markdown = "- fixtures/downstream/no-default-features/Cargo.toml: package 0.0.0";

    let bounds = markdown_stream_bounds(markdown, 39, false);

    assert_eq!(bounds.drain.byte_index, 39);
    assert!(bounds.drain.ends_with_wrap);
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

#[test]
fn image_syntax_inside_code_fence_stays_literal() {
    let mut in_code_block = false;
    let lines = markdown_lines("```\n![diagram](arch.png)\n```", 120, &mut in_code_block);
    let text: Vec<String> = lines.iter().map(line_text).collect();

    assert!(text
        .iter()
        .any(|line| line.contains("![diagram](arch.png)")));
}

#[test]
fn stable_prefix_stops_at_earliest_open_marker() {
    assert_eq!(
        inline_markdown_stable_prefix_len("before **bold"),
        "before ".len()
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("before *it **also"),
        "before ".len(),
        "earlier italic must win over later bold"
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("before **bold `code"),
        "before ".len(),
        "earlier bold must win over later code"
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("before `code **bold"),
        "before ".len()
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("a _x **y"),
        "a ".len(),
        "earlier underscore italic must win over later bold"
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("done **x** tail"),
        "done **x** tail".len()
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("text *a* **b"),
        "text *a* ".len()
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("see [x](http"),
        "see ".len()
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("see [x]"),
        "see ".len(),
        "trailing ] may still become a link target"
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("see [x] plain"),
        "see [x] plain".len(),
        "] followed by non-( stays plain text"
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("go https://ex"),
        "go ".len()
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("before [link](https://x) **b"),
        "before [link](https://x) ".len()
    );
}

// Covers: closed $$ blocks must render through the markdown panel path
// Owner: pure unit (markdown display math integration)
#[test]
fn renders_closed_display_math_blocks_in_markdown() {
    let mut in_code_block = false;
    let multi = markdown_lines(
        "before\n$$\n\\frac{a}{b}\n$$\nafter",
        40,
        &mut in_code_block,
    );
    let multi_text = multi.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        multi_text.iter().any(|line| line.contains("MATH")),
        "{multi_text:?}"
    );
    assert!(
        multi_text
            .iter()
            .any(|line| line.contains('a') && !line.contains("\\frac")),
        "{multi_text:?}"
    );
    assert!(
        multi_text.iter().any(|line| line == "after"),
        "{multi_text:?}"
    );
    assert!(!in_code_block);

    let mut in_code_block = false;
    let single = markdown_lines("$$x^2 + y^2$$", 40, &mut in_code_block);
    let single_text = single.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        single_text.iter().any(|line| line.contains("MATH")),
        "{single_text:?}"
    );
    assert!(
        !single_text.iter().any(|line| line.contains("$$")),
        "{single_text:?}"
    );
}

// Covers: $$ inside fences and open $$ tails must not false-commit
// Owner: pure unit (markdown display math streaming bounds)
#[test]
fn keeps_fenced_and_open_display_math_literal() {
    let mut in_code_block = false;
    let fenced = markdown_lines("```text\n$$x^2$$\n```", 40, &mut in_code_block);
    let fenced_text = fenced.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        fenced_text.iter().any(|line| line.contains("$$x^2$$")),
        "{fenced_text:?}"
    );
    assert!(
        !fenced_text.iter().any(|line| line.contains("MATH")),
        "{fenced_text:?}"
    );

    let open = "intro\n$$\n\\frac{a}{b}";
    assert_eq!(
        incremental_markdown_tail_start(open),
        "intro\n".len(),
        "open multi-line math must stay in the mutable tail"
    );
    let closed_then_prose = "intro\n$$\n\\frac{a}{b}\n$$\nafter";
    assert_eq!(
        incremental_markdown_tail_start(closed_then_prose),
        "intro\n$$\n\\frac{a}{b}\n$$\n".len(),
        "prose after a closed math block becomes the trailing block"
    );
}
