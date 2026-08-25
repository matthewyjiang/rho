use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use ratatui::{layout::Rect, text::Line};

use super::{agent_activity, SubagentPanel, SubagentPointerTarget};
use crate::{
    subagent::{RunState, RunStatus},
    tools::agent::SubagentSnapshot,
    tui::{activity, theme::Theme},
};

fn snapshot(id: &str, agent_id: &str, state: RunState, elapsed_seconds: u64) -> SubagentSnapshot {
    SubagentSnapshot {
        id: id.to_owned(),
        agent_id: agent_id.to_owned(),
        title: None,
        elapsed: Duration::from_secs(elapsed_seconds),
        done: state.is_terminal(),
        status: RunStatus {
            state,
            last_activity: Some("read".into()),
            ..RunStatus::default()
        },
    }
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn activity_span_style(line: &Line<'_>, activity: &str) -> ratatui::style::Style {
    line.spans
        .iter()
        .find(|span| span.content.as_ref() == activity)
        .map(|span| span.style)
        .unwrap_or_default()
}

// Covers: subagent verdict labels and styles are exhaustive on RunState.
// Owner: pure unit (subagent rail labels)
#[test]
fn subagent_verdict_labels_and_styles_match_state() {
    let starting = super::RunningSubagent {
        id: "a".into(),
        agent_id: "worker".into(),
        title: None,
        state: RunState::Starting,
        last_activity: None,
        elapsed_seconds: 1,
    };
    let cases = [
        (RunState::Starting, "starting", Theme::text()),
        (RunState::Ok, "✓ done", Theme::activity_rail_success()),
        (RunState::Error, "✗ error", Theme::activity_rail_error()),
        (RunState::Stopped, "✗ stopped", Theme::dim()),
    ];
    for (state, label, style) in cases {
        let mut agent = starting.clone();
        agent.state = state;
        assert_eq!(agent_activity(&agent), (label.to_owned(), style));
    }
}

// Covers: a just-finished agent stays through the linger window, then drops.
// Owner: pure unit (subagent linger)
#[test]
fn subagent_linger_keeps_then_drops_around_deadline() {
    let mut panel = SubagentPanel::default();
    let t0 = Instant::now();
    assert!(panel.ingest(vec![snapshot("aa0001", "worker", RunState::Running, 4)], t0,));
    assert_eq!(panel.count(), 1);

    assert!(panel.ingest(vec![snapshot("aa0001", "worker", RunState::Ok, 4)], t0));
    assert_eq!(panel.count(), 0);
    assert!(panel.is_active());
    let kept = line_text(&panel.lines(80, 8, "attach", false, t0)[0]);
    assert!(kept.contains("✓ done"));
    assert!(kept.contains(activity::AGENT_GLYPH));

    let before = t0 + activity::LINGER_OK - Duration::from_millis(1);
    assert!(!panel.ingest(vec![snapshot("aa0001", "worker", RunState::Ok, 4)], before,));
    assert_eq!(
        panel
            .lines(80, 8, "attach", false, t0 + activity::LINGER_OK)
            .len(),
        0
    );
    assert!(panel.ingest(
        vec![snapshot("aa0001", "worker", RunState::Ok, 4)],
        t0 + activity::LINGER_OK
    ));
    assert!(!panel.is_active());
}

// Covers: spinner agent count ignores lingering rows.
// Owner: pure unit (subagent count)
#[test]
fn count_excludes_lingering_rows() {
    let mut panel = SubagentPanel::default();
    let now = Instant::now();
    panel.ingest(
        vec![
            snapshot("live01", "worker", RunState::Running, 2),
            snapshot("done01", "explorer", RunState::Running, 3),
        ],
        now,
    );
    panel.ingest(
        vec![
            snapshot("live01", "worker", RunState::Running, 2),
            snapshot("done01", "explorer", RunState::Error, 3),
        ],
        now,
    );
    assert_eq!(panel.count(), 1);
    assert_eq!(panel.desired_height(), 2);
    assert!(panel.candidates().iter().all(|c| c.run_id == "live01"));
}

// Covers: overflow copy is singular/plural and is the attach-picker target.
// Owner: pure unit (overflow copy + pointer)
#[test]
fn subagent_overflow_summary_opens_attach_picker() {
    let mut panel = SubagentPanel::default();
    let now = Instant::now();
    panel.ingest(
        vec![
            snapshot("aa0001", "worker", RunState::Running, 1),
            snapshot("aa0002", "explorer", RunState::Running, 1),
            snapshot("aa0003", "reviewer", RunState::Running, 1),
        ],
        now,
    );
    let lines = panel.lines(80, 8, "attach", false, now);
    assert!(line_text(&lines[1]).contains("2 more agents"));
    assert!(line_text(&lines[1]).contains("/attach"));

    let area = Rect::new(0, 0, 80, 2);
    assert_eq!(
        panel.attach_target_at(area, 1, 1, now),
        Some(SubagentPointerTarget::OpenAttachPicker)
    );
    assert!(matches!(
        panel.attach_target_at(area, 1, 0, now),
        Some(SubagentPointerTarget::Run(_))
    ));
}

// Covers: lingering rows occupy height but are not attachable.
// Owner: pure unit (attach gating)
#[test]
fn lingering_subagent_rows_are_not_clickable() {
    let mut panel = SubagentPanel::default();
    let now = Instant::now();
    panel.ingest(
        vec![snapshot("aa0001", "worker", RunState::Running, 4)],
        now,
    );
    panel.ingest(vec![snapshot("aa0001", "worker", RunState::Ok, 4)], now);
    let area = Rect::new(0, 0, 80, 1);
    assert_eq!(panel.attach_target_at(area, 1, 0, now), None);
    assert!(panel.candidates().is_empty());
}

// Covers: hover trailing keeps elapsed instead of replacing it.
// Owner: pure layout
#[test]
fn hover_trailing_keeps_elapsed() {
    let mut panel = SubagentPanel::default();
    let now = Instant::now();
    panel.ingest(
        vec![snapshot("aa0001", "worker", RunState::Running, 4)],
        now,
    );
    panel.set_hovered(Some("aa0001"));
    let text = line_text(&panel.lines(80, 8, "attach", false, now)[0]);
    assert!(text.contains("⏎ attach · 4s"));
    assert_eq!(
        panel.highlighted_row(now),
        Some((0, activity::RailRowState::Hovered))
    );
}

// Covers: success/error verdict styles paint on wide rows.
// Owner: pure layout
#[test]
fn subagent_verdict_styles_paint_on_wide_rows() {
    let mut panel = SubagentPanel::default();
    let now = Instant::now();
    panel.ingest(
        vec![snapshot("aa0001", "worker", RunState::Running, 4)],
        now,
    );
    panel.ingest(vec![snapshot("aa0001", "worker", RunState::Error, 4)], now);
    let line = &panel.lines(80, 8, "attach", false, now)[0];
    assert_eq!(
        activity_span_style(line, "✗ error"),
        Theme::activity_rail().patch(Theme::activity_rail_error())
    );
}
