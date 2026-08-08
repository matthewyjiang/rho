use pretty_assertions::assert_eq;
use rho_tools::tool_card::{
    DiffRow, DiffRowKind, ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus,
};

use super::{card_is_toggleable, push_tool_card};
use crate::tui::{
    syntax::{reset_highlight_line_calls, take_highlight_line_calls, warm_syntax_set},
    theme::{SyntaxRole, Theme},
};

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn render(card: &ToolCard, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    push_tool_card(
        &mut lines, card, width, /*max_tool_output_lines*/ 32, /*expanded*/ true,
    );
    lines.into_iter().map(|line| line_text(&line)).collect()
}

// Covers: wrapped fact rows keep a tree stem so long child text stays tied to the trunk.
// Owner: pure TUI layout
#[test]
fn mid_fact_wrap_keeps_box_stem_before_later_sibling() {
    let mut card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Web,
        ToolHeader::call("x_search", None),
    );
    card.push_fact(ToolFact::Text {
        text: "Cerebras (5.6 OR o3) coming or release soon".into(),
    });
    card.push_fact(ToolFact::Meta {
        text: "finished".into(),
    });

    let lines = render(&card, 28);
    assert_eq!(lines[0], "✓ x_search");
    assert!(
        lines[1].starts_with("  ├ "),
        "first fact row should branch: {:?}",
        lines
    );
    assert!(
        lines.iter().any(|line| line.starts_with("  │ ")),
        "wrapped mid fact should extend with │: {:?}",
        lines
    );
    assert_eq!(lines.last().map(String::as_str), Some("  └ finished"));
}

// Covers: last fact wrap must not leave a dangling │ under └.
// Owner: pure TUI layout
#[test]
fn last_fact_wrap_uses_space_hang_not_stem() {
    let mut card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Web,
        ToolHeader::call("x_search", None),
    );
    card.push_fact(ToolFact::Text {
        text: "only child query that needs several wraps at this width".into(),
    });

    let lines = render(&card, 24);
    assert!(
        lines[1].starts_with("  └ "),
        "sole fact should be last branch: {:?}",
        lines
    );
    let continuations: Vec<_> = lines.iter().skip(2).collect();
    assert!(
        !continuations.is_empty(),
        "expected wrap continuations: {:?}",
        lines
    );
    for line in continuations {
        assert!(
            line.starts_with("    ") && !line.starts_with("  │ "),
            "last-child wrap must hang with spaces: {:?}",
            lines
        );
    }
}

// Covers: multi-file File headers keep a continuous trunk through body rows so
// section branches read as one tree (│ under mid files, hang under the last).
// Owner: pure TUI layout
#[test]
fn multi_file_diff_connects_body_under_section_headers() {
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileDiff,
        ToolHeader::call("edit", Some("2 files".into())),
    )
    .with_body(ToolBody::Diff(vec![
        DiffRow::file_header("a.txt", Some((1, 1))),
        DiffRow::new(DiffRowKind::Added, Some(1), "A"),
        DiffRow::file_header("b.txt", Some((0, 1))),
        DiffRow::new(DiffRowKind::Removed, Some(1), "B"),
    ]));

    let lines = render(&card, 40);
    assert_eq!(lines[0], "✓ edit(2 files)");
    assert!(
        lines[1].starts_with("  ├ ")
            && lines[1].contains("+1 -1 lines")
            && lines[1].contains("a.txt"),
        "first file section should mid-branch: {:?}",
        lines
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("  │ ") && line.contains("A")),
        "body under first file must keep trunk stem: {:?}",
        lines
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("  └ ") && line.contains("b.txt")),
        "last file section should end-branch: {:?}",
        lines
    );
    assert!(
        lines.iter().any(|line| {
            line.contains("B") && line.starts_with("    ") && !line.starts_with("  │ ")
        }),
        "body under last file must hang without stem: {:?}",
        lines
    );
}

// Covers: fact wrap prefers whitespace over hard mid-word cuts (same as headers).
// Owner: pure TUI layout
#[test]
fn fact_wrap_breaks_on_whitespace() {
    let mut card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Default,
        ToolHeader::call("tool", None),
    );
    card.push_fact(ToolFact::Text {
        text: "one two three four".into(),
    });

    // prefix "  └ " is 4 cols; content width 10.
    // Soft wrap: "one two" / "three four". Hard wrap would cut "three".
    let lines = render(&card, 14);
    let joined = lines.join("\n");
    assert!(
        !joined.contains("one two th"),
        "must not hard-split inside 'three': {:?}",
        lines
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("three four") || line.ends_with("three")),
        "expected whitespace-bounded wrap rows: {:?}",
        lines
    );
}

// Covers: write/edit diff bodies syntax-highlight from the header path
// Owner: pure TUI (tool card diff highlighting)
#[test]
fn file_diff_body_highlights_rust_from_header_path() {
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileDiff,
        ToolHeader::call("write", Some("src/lib.rs".into())),
    )
    .with_body(ToolBody::Diff(vec![DiffRow::new(
        DiffRowKind::Added,
        Some(1),
        "let answer = 42; // note",
    )]));

    let mut lines = Vec::new();
    push_tool_card(
        &mut lines, &card, /*width*/ 80, /*max_tool_output_lines*/ 32,
        /*expanded*/ true,
    );
    let body = lines
        .iter()
        .find(|line| line.spans.iter().any(|span| span.content.contains("let")))
        .expect("diff body row");

    assert!(
        body.spans.iter().any(|span| {
            span.content.contains("let") && span.style == Theme::syntax(SyntaxRole::Keyword)
        }),
        "expected keyword highlight in spans: {:?}",
        body.spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect::<Vec<_>>()
    );
    assert!(
        body.spans.iter().any(|span| {
            span.content.contains('+') && span.style == Theme::tool_diff_text(DiffRowKind::Added)
        }),
        "expected green add sign: {:?}",
        body.spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect::<Vec<_>>()
    );
}

fn large_rust_diff_card(lines: usize) -> ToolCard {
    let rows = (1..=lines)
        .map(|i| {
            DiffRow::new(
                DiffRowKind::Added,
                Some(i as u32),
                format!("let value_{i} = {i}; // line {i}"),
            )
        })
        .collect();
    ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileDiff,
        ToolHeader::call("write", Some("src/big.rs".into())),
    )
    .with_body(ToolBody::Diff(rows))
}

// Covers: collapsed paint must not language-highlight the whole write body
// Owner: pure unit (tool card highlight budget)
#[test]
fn collapsed_diff_paint_highlights_only_budget_rows() {
    warm_syntax_set();
    let card = large_rust_diff_card(200);
    let budget = 10usize;

    reset_highlight_line_calls();
    let mut collapsed = Vec::new();
    push_tool_card(
        &mut collapsed,
        &card,
        /*width*/ 100,
        budget,
        /*expanded*/ false,
    );
    let collapsed_calls = take_highlight_line_calls();

    reset_highlight_line_calls();
    let mut expanded = Vec::new();
    push_tool_card(
        &mut expanded,
        &card,
        /*width*/ 100,
        budget,
        /*expanded*/ true,
    );
    let expanded_calls = take_highlight_line_calls();

    assert!(
        collapsed_calls <= budget + 2,
        "collapsed should paint ~budget lines, got {collapsed_calls}"
    );
    assert!(
        expanded_calls >= 150,
        "expanded should paint most of the body, got {expanded_calls}"
    );
    assert!(
        collapsed_calls * 5 < expanded_calls,
        "collapsed ({collapsed_calls}) should be much cheaper than expanded ({expanded_calls})"
    );
}

// Covers: toggle check must not run syntect
// Owner: pure unit (tool card toggle estimate)
#[test]
fn toggle_check_does_not_highlight() {
    warm_syntax_set();
    let card = large_rust_diff_card(120);
    reset_highlight_line_calls();
    assert!(card_is_toggleable(
        &card, /*width*/ 100, /*max_tool_output_lines*/ 10, /*expanded*/ false,
    ));
    assert_eq!(
        take_highlight_line_calls(),
        0,
        "toggle check must stay highlight-free"
    );
}

// Covers: wall-clock collapsed paint is faster than expanded (release check)
// Owner: pure unit (tool card highlight perf)
#[test]
fn collapsed_paint_is_faster_than_expanded_wall_clock() {
    use std::time::Instant;

    warm_syntax_set();
    let card = large_rust_diff_card(600);
    let budget = 10usize;
    let width = 100usize;
    let rounds = 5usize;

    // Warm-up
    for _ in 0..2 {
        let mut lines = Vec::new();
        push_tool_card(&mut lines, &card, width, budget, false);
        push_tool_card(&mut lines, &card, width, budget, true);
        let _ = card_is_toggleable(&card, width, budget, false);
    }

    let mut collapsed = std::time::Duration::ZERO;
    let mut expanded = std::time::Duration::ZERO;
    let mut toggle = std::time::Duration::ZERO;
    let mut collapsed_calls = 0usize;
    let mut expanded_calls = 0usize;

    for _ in 0..rounds {
        reset_highlight_line_calls();
        let start = Instant::now();
        let mut lines = Vec::new();
        push_tool_card(&mut lines, &card, width, budget, false);
        collapsed += start.elapsed();
        collapsed_calls += take_highlight_line_calls();
        std::hint::black_box(lines);

        reset_highlight_line_calls();
        let start = Instant::now();
        let mut lines = Vec::new();
        push_tool_card(&mut lines, &card, width, budget, true);
        expanded += start.elapsed();
        expanded_calls += take_highlight_line_calls();
        std::hint::black_box(lines);

        let start = Instant::now();
        let ok = card_is_toggleable(&card, width, budget, false);
        toggle += start.elapsed();
        std::hint::black_box(ok);
    }

    let c_ms = collapsed.as_secs_f64() * 1000.0 / rounds as f64;
    let e_ms = expanded.as_secs_f64() * 1000.0 / rounds as f64;
    let t_ms = toggle.as_secs_f64() * 1000.0 / rounds as f64;
    let c_calls = collapsed_calls / rounds;
    let e_calls = expanded_calls / rounds;
    eprintln!(
        "tool_card highlight bench (600-line write, budget={budget}): \
         collapsed={c_ms:.3}ms/{c_calls} calls, expanded={e_ms:.3}ms/{e_calls} calls, \
         toggle={t_ms:.3}ms, speedup={:.1}x",
        e_ms / c_ms.max(0.001)
    );

    assert!(
        c_calls * 10 < e_calls,
        "collapsed calls ({c_calls}) should be << expanded ({e_calls})"
    );
    // Wall clock can jitter in debug CI; only enforce a soft ratio in release.
    if cfg!(not(debug_assertions)) {
        assert!(
            e_ms > c_ms * 3.0,
            "release collapsed paint should be >=3x faster: collapsed={c_ms:.3}ms expanded={e_ms:.3}ms"
        );
    }
}

// Covers: grep body gets language roles and match overlay from match_pattern
// Owner: pure TUI (grep search highlight)
#[test]
fn grep_body_highlights_language_and_match() {
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileCommand,
        ToolHeader::call("grep", Some("answer, src".into())),
    )
    .with_match_pattern("answer")
    .with_body(ToolBody::Lines(vec![
        "src/lib.rs".into(),
        "1 | let answer = 42;".into(),
        "1 matches in 1 file".into(),
    ]));

    let mut lines = Vec::new();
    push_tool_card(
        &mut lines, &card, /*width*/ 80, /*max_tool_output_lines*/ 32,
        /*expanded*/ true,
    );
    let body = lines
        .iter()
        .find(|line| line.spans.iter().any(|span| span.content.contains("let")))
        .expect("grep content row");
    assert!(
        body.spans.iter().any(|span| {
            span.content.contains("let") && span.style == Theme::syntax(SyntaxRole::Keyword)
        }),
        "expected rust keyword: {:?}",
        body.spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect::<Vec<_>>()
    );
    assert!(
        body.spans.iter().any(|span| {
            span.content.as_ref() == "answer" && span.style == Theme::search_match(Theme::text())
        }),
        "expected match overlay: {:?}",
        body.spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect::<Vec<_>>()
    );
}
