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
        let mut fence_state = CodeFenceState::default();
        let lines = markdown_lines(&markdown, 39, &mut fence_state);

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
    let mut fence_state = CodeFenceState::default();
    let lines = markdown_lines(
        "keep foo_bar_baz literal but style _this_",
        120,
        &mut fence_state,
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
    let mut fence_state = CodeFenceState::default();

    let lines = markdown_lines(&markdown, 17, &mut fence_state);
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
fn code_block_rows_use_the_full_pane_width_without_borders() {
    let mut fence_state = CodeFenceState::default();
    let lines = markdown_lines("```\n你好你好\n```", 6, &mut fence_state);

    // Header row plus content rows, no bottom border.
    assert_eq!(lines.len(), 3);
    assert_eq!(line_text(&lines[1]), "你好你");
    assert_eq!(line_text(&lines[2]), "好");
    assert!(lines
        .iter()
        .all(|line| !line_text(line).contains(['╭', '╮', '╰', '╯', '│'])));
}

#[test]
fn code_blocks_preserve_markdown_markers_as_literal_text() {
    let mut fence_state = CodeFenceState::default();
    let lines = markdown_lines(
        "```\nfn __init__() { println!(\"*ok*\"); }\n```",
        80,
        &mut fence_state,
    );

    assert!(line_text(&lines[1]).contains("fn __init__() { println!(\"*ok*\"); }"));
    assert_eq!(line_styles(&lines[1]), vec![Theme::code_text()]);
}

#[test]
fn code_block_header_shows_language_label_and_copy_button() {
    let mut fence_state = CodeFenceState::default();
    let rendered = render_markdown("```rust\nlet x = 1;\n```", 40, &mut fence_state);

    let header = &rendered.lines[0];
    // COPY keeps one blank column of inset from the right pane edge.
    assert_eq!(display_width(&line_text(header)), 39);
    assert!(line_text(header).starts_with("RUST"));
    assert!(line_text(header).ends_with(" COPY "));
    assert!(line_styles(header).contains(&Theme::dim()));
    assert!(line_styles(header).contains(&Theme::markdown_code_copy_button(/*hovered*/ false)));
}

#[test]
fn highlighted_code_blocks_style_tokens_and_keep_literal_text() {
    let mut fence_state = CodeFenceState::default();
    let lines = markdown_lines(
        "```rust\nlet answer = 42; // note\n```",
        80,
        &mut fence_state,
    );

    assert_eq!(line_text(&lines[1]), "let answer = 42; // note");
    let styles = line_styles(&lines[1]);
    assert!(styles.len() > 1, "expected highlighted spans: {styles:?}");
    assert!(styles.iter().any(|style| *style != Theme::code_text()));
}

#[test]
fn unknown_language_code_blocks_fall_back_to_plain_styling() {
    let mut fence_state = CodeFenceState::default();
    let lines = markdown_lines(
        "```no-such-language\nplain text body\n```",
        80,
        &mut fence_state,
    );

    assert!(line_text(&lines[0]).starts_with("NO-SUCH-LANGUAGE"));
    assert_eq!(line_text(&lines[1]), "plain text body");
    assert_eq!(line_styles(&lines[1]), vec![Theme::code_text()]);
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
fn streamed_code_fence_state_preserves_marker_length_and_language_across_chunks() {
    let mut state = CodeFenceState::default();
    update_code_block_state("````mermaid\nflowchart TD", &mut state);
    assert!(state.is_open());
    assert_eq!(state.language.as_deref(), Some("mermaid"));
    update_code_block_state("```", &mut state);
    assert!(state.is_open());
    assert_eq!(state.language.as_deref(), Some("mermaid"));
    update_code_block_state("````", &mut state);
    assert!(!state.is_open());
    assert_eq!(state.language, None);

    update_code_block_state("~~~~rust", &mut state);
    assert!(state.is_open());
    assert_eq!(state.language.as_deref(), Some("rust"));
    update_code_block_state("```", &mut state);
    assert!(state.is_open());
    update_code_block_state("~~~~", &mut state);
    assert!(!state.is_open());
    assert_eq!(state.language, None);
}

// Covers: live preview continuation lines must highlight when fence language
// is carried on CodeFenceState from an earlier opening chunk.
// Owner: pure unit (markdown fence-state render path)
#[test]
fn fence_state_continuation_highlights_with_carried_language() {
    let mut state = CodeFenceState::default();
    update_code_block_state("```rust\n", &mut state);
    assert!(state.is_open());
    assert_eq!(state.language.as_deref(), Some("rust"));

    // Body-only chunk, as the live preview receives after the opening fence
    // has already been committed above.
    let lines = markdown_lines("let answer = 42; // note", 80, &mut state);
    assert_eq!(lines.len(), 1);
    assert_eq!(line_text(&lines[0]), "let answer = 42; // note");
    let styles = line_styles(&lines[0]);
    assert!(
        styles.len() > 1,
        "continuation must be highlighted, got {styles:?}"
    );
    assert!(styles.iter().any(|style| *style != Theme::code_text()));
    assert!(state.is_open());
}

#[test]
fn mermaid_scanner_keeps_an_invalid_closer_inside_the_raw_block() {
    let mut fence_state = CodeFenceState::default();
    let rendered = render_markdown(
        "````mermaid\nflowchart TD\nA[one]\n```not-a-close\nA --> B[two]\n````",
        80,
        &mut fence_state,
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
    let mut fence_state = CodeFenceState::default();
    let open = render_markdown("```mermaid\nflowchart LR\nA --> B", 60, &mut fence_state);
    let open_text = open.lines.iter().map(line_text).collect::<Vec<_>>();

    assert!(fence_state.is_open());
    // The header labels the open block, but the source stays raw until closed.
    assert!(open_text[0].starts_with("MERMAID"));
    assert!(open_text.iter().any(|line| line.contains("flowchart LR")));

    let mut fence_state = CodeFenceState::default();
    let closed = render_markdown(
        "```mermaid\nflowchart LR\nA --> B\n```",
        60,
        &mut fence_state,
    );
    assert!(!fence_state.is_open());
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
    let mut wide_state = CodeFenceState::default();
    let wide = markdown_lines(markdown, 80, &mut wide_state);
    let mut narrow_state = CodeFenceState::default();
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
    let mut fence_state = CodeFenceState::default();
    let lines = markdown_lines("```\n![diagram](arch.png)\n```", 120, &mut fence_state);
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

// Covers: open $ math must hold the preview while currency dollars stay plain
// Owner: pure unit (markdown inline math streaming bounds)
#[test]
fn stable_prefix_holds_open_inline_math_but_not_currency() {
    assert_eq!(
        inline_markdown_stable_prefix_len("before $x^2"),
        "before ".len()
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("pay $"),
        "pay ".len(),
        "a trailing $ may still open math once more input arrives"
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("done $x^2$ tail"),
        "done $x^2$ tail".len()
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("cost $5 and more"),
        "cost $5 and more".len(),
        "a $ before a digit is currency, not math"
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("a `$x` b"),
        "a `$x` b".len(),
        "$ inside a code span never opens math"
    );
    assert_eq!(
        inline_markdown_stable_prefix_len("closed $a_i$ then *em"),
        "closed $a_i$ then ".len(),
        "markers after closed math must still hold"
    );
}

// Covers: closed $...$ renders single-row art while currency and tall math stay literal
// Owner: pure unit (markdown inline math integration)
#[test]
fn renders_single_row_inline_math_in_prose() {
    let mut fence_state = CodeFenceState::default();
    let lines = markdown_lines("energy $E = mc^2$ done", 80, &mut fence_state);
    let text = lines.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(text, vec!["energy E = mc² done"]);

    let mut fence_state = CodeFenceState::default();
    let currency = markdown_lines("that costs $5 and $10 total", 80, &mut fence_state);
    let currency_text = currency.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(currency_text, vec!["that costs $5 and $10 total"]);

    let mut fence_state = CodeFenceState::default();
    let tall = markdown_lines(r"half is $\frac{1}{2}$ here", 80, &mut fence_state);
    let tall_text = tall.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(
        tall_text,
        vec![r"half is $\frac{1}{2}$ here"],
        "multi-row inline math must keep its literal source"
    );

    let mut fence_state = CodeFenceState::default();
    let code = markdown_lines("run `echo $x^2$` now", 80, &mut fence_state);
    let code_text = code.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        code_text.iter().any(|line| line.contains("echo $x^2$")),
        "code spans keep dollars literal: {code_text:?}"
    );
}

// Covers: closed $$ blocks must render through the markdown panel path
// Owner: pure unit (markdown display math integration)
#[test]
fn renders_closed_display_math_blocks_in_markdown() {
    let mut fence_state = CodeFenceState::default();
    let multi = markdown_lines("before\n$$\n\\frac{a}{b}\n$$\nafter", 40, &mut fence_state);
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
    assert!(!fence_state.is_open());

    let mut fence_state = CodeFenceState::default();
    let single = markdown_lines("$$x^2 + y^2$$", 40, &mut fence_state);
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
    let mut fence_state = CodeFenceState::default();
    let fenced = markdown_lines("```text\n$$x^2$$\n```", 40, &mut fence_state);
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
