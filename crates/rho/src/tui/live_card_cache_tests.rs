use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use rho_tools::tool_card::{
    DiffRow, DiffRowKind, ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus,
};

use crate::tui::{
    syntax::{reset_highlight_line_calls, take_highlight_line_calls, warm_syntax_set},
    theme::Theme,
    ToolEntry,
};

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn rust_diff_entry(lines: usize, expanded: bool) -> ToolEntry {
    let rows = (1..=lines)
        .map(|i| {
            DiffRow::new(
                DiffRowKind::Added,
                Some(i as u32),
                format!("let value_{i} = {i}; // line {i}"),
            )
        })
        .collect();
    ToolEntry::new(
        ToolCard::new(
            ToolStatus::Running,
            ToolFamily::FileDiff,
            ToolHeader::call("write", Some("src/big.rs".into())),
        )
        .with_body(ToolBody::Diff(rows)),
        expanded,
        None,
        None,
    )
}

fn running_shell(started_at: Instant) -> ToolEntry {
    let mut card = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::FileCommand,
        ToolHeader::shell("$", Some("sleep 1".into())),
    );
    card.push_fact(ToolFact::Timeout { seconds: Some(30) });
    ToolEntry::new(card, true, None, Some(started_at))
}

// Covers: unchanged live card must not re-highlight on a later frame.
// Owner: tui live card render cache
#[test]
fn live_card_cache_hits_skip_syntax_highlight() {
    let _guard = crate::tui::theme::theme_test_lock();
    warm_syntax_set();
    let mut entry = rust_diff_entry(40, true);
    reset_highlight_line_calls();
    let first = entry
        .rendered_lines(80, 32, 4)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    let first_calls = take_highlight_line_calls();
    assert!(first_calls > 0, "first paint must highlight: {first_calls}");
    assert_eq!(entry.render_cache_paints(), 1);

    reset_highlight_line_calls();
    let second = entry
        .rendered_lines(80, 32, 4)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    assert_eq!(take_highlight_line_calls(), 0);
    assert_eq!(entry.render_cache_paints(), 1);
    assert_eq!(first, second);
}

// Covers: themed live-card cache must miss after Theme::generation() bumps.
// Owner: tui live card render cache
#[test]
fn live_card_cache_misses_after_theme_generation_change() {
    let _guard = crate::tui::theme::theme_test_lock();
    Theme::apply_committed("terminal");
    let mut entry = rust_diff_entry(8, false);
    let first = entry
        .rendered_lines(40, 8, 4)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    Theme::apply_committed("one-half-light");
    let generation = Theme::generation();
    let second = entry
        .rendered_lines(40, 8, 4)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    let cached_generation = entry.render_cache_theme_generation();
    Theme::apply_committed("terminal");
    assert_eq!(first, second);
    assert_eq!(cached_generation, Some(generation));
    assert_eq!(entry.render_cache_paints(), 2);
}

// Covers: expand/collapse and width changes rebuild; elapsed ticks reuse the body.
// Owner: tui live card render cache
#[test]
fn live_card_cache_invalidates_on_expand_and_patches_elapsed() {
    let _guard = crate::tui::theme::theme_test_lock();
    warm_syntax_set();
    let mut entry = rust_diff_entry(40, false);
    entry.rendered_lines(80, 8, 4);
    assert_eq!(entry.render_cache_paints(), 1);
    entry.expanded = true;
    entry.rendered_lines(80, 8, 4);
    assert_eq!(entry.render_cache_paints(), 2);
    entry.rendered_lines(60, 8, 4);
    assert_eq!(entry.render_cache_paints(), 3);

    let started = Instant::now() - Duration::from_millis(1_200);
    let mut shell = running_shell(started);
    reset_highlight_line_calls();
    let first = shell
        .rendered_lines(60, 8, 4)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    assert!(
        first.iter().any(|line| line.contains("timeout 30s · 1.2s")),
        "first paint must include elapsed: {first:?}"
    );
    let paints = shell.render_cache_paints();
    shell.started_at = Some(Instant::now() - Duration::from_millis(1_300));
    let second = shell
        .rendered_lines(60, 8, 4)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    assert_eq!(shell.render_cache_paints(), paints);
    assert!(
        second
            .iter()
            .any(|line| line.contains("timeout 30s · 1.3s")),
        "elapsed tick must rebuild prefix without a full paint: {second:?}"
    );
}
