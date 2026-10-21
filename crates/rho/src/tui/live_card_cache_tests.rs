use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use rho_tools::tool_card::{
    DiffRow, DiffRowKind, ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus,
};

use crate::tui::{
    syntax::{reset_highlight_line_calls, take_highlight_line_calls, warm_syntax_set},
    theme::Theme,
    tool_output_ui::tool_output_toggleable,
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

// Covers: toggle metadata follows wrapped child height, not visible/clipped rows,
// and cannot be reused for another terminal width or output budget.
// Owner: pure unit (live card layout cache); no interactive behavior is asserted.
#[test]
fn cached_toggleability_tracks_layout_and_survives_expansion() {
    for (body, width, budget, expected) in [
        (vec![], 14, 1, false),
        (vec![""], 14, 1, false),
        (vec!["abcdefgh"], 14, 1, false),
        (vec!["abcdefghi"], 14, 1, true),
        (vec!["a\nb", ""], 14, 2, true),
        (vec!["a\nb", ""], 14, 3, false),
    ] {
        let mut entry = ToolEntry::new(
            ToolCard::new(
                ToolStatus::Ok,
                ToolFamily::FileCommand,
                ToolHeader::call("read", None),
            )
            .with_body(ToolBody::Lines(
                body.into_iter().map(str::to_string).collect(),
            )),
            /*expanded*/ false,
            None,
            None,
        );
        assert_eq!(entry.cached_output_toggleable(width, budget), None);
        assert_eq!(tool_output_toggleable(&entry, budget, width), expected);
        entry.rendered_lines(width, budget, 4);
        assert_eq!(
            entry.cached_output_toggleable(width, budget),
            Some(expected)
        );
        assert_eq!(tool_output_toggleable(&entry, budget, width), expected);

        entry.expanded = true;
        assert_eq!(
            entry.cached_output_toggleable(width, budget),
            Some(expected)
        );
        entry.rendered_lines(width, budget, 4);
        assert_eq!(
            entry.cached_output_toggleable(width, budget),
            Some(expected)
        );
        assert_eq!(entry.cached_output_toggleable(width + 1, budget), None);
        assert_eq!(entry.cached_output_toggleable(width, budget + 1), None);
        assert_eq!(entry.clone().cached_output_toggleable(width, budget), None);
    }
}

// Covers: themed live-card cache must miss after Theme::generation() bumps.
// Owner: tui live card render cache
#[test]
fn live_card_cache_misses_after_theme_generation_change() {
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
    assert!(take_highlight_line_calls() > 0);
    let paints = shell.render_cache_paints();
    shell.started_at = Some(Instant::now() - Duration::from_millis(1_300));
    reset_highlight_line_calls();
    let second = shell
        .rendered_lines(60, 8, 4)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    assert_eq!(shell.render_cache_paints(), paints);
    assert_eq!(take_highlight_line_calls(), 0);
    assert!(
        second
            .iter()
            .any(|line| line.contains("timeout 30s · 1.3s")),
        "elapsed tick must rebuild prefix without a full paint: {second:?}"
    );
}
