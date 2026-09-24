use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use ratatui::{layout::Rect, text::Line};

use super::{
    command_identity, process_activity, process_trailing_style, ProcessPanel, ProcessPeekTarget,
    QUIET_WARN_AFTER,
};
use crate::{
    tools::process::{LiveProcessSummary, State},
    tui::{
        activity,
        theme::{self, Theme},
    },
};

fn summary(id: &str, command: &str, elapsed_seconds: u64) -> LiveProcessSummary {
    LiveProcessSummary {
        process_id: id.to_owned(),
        command: command.to_owned(),
        state: State::Running,
        elapsed_seconds,
        quiet_seconds: None,
        exit_code: None,
    }
}

fn with_state(mut process: LiveProcessSummary, state: State) -> LiveProcessSummary {
    process.state = state;
    process
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

// Covers: more than two live processes must not grow the rail past the shared cap.
// Owner: pure layout
#[test]
fn desired_height_caps_at_two_and_overflow_summarizes() {
    let mut panel = ProcessPanel::default();
    let now = Instant::now();
    assert!(panel.ingest(
        vec![
            summary("aaaaaaaa-1111", "sleep 1", 3),
            summary("bbbbbbbb-2222", "sleep 2", 2),
            summary("cccccccc-3333", "sleep 3", 1),
        ],
        now,
    ));

    assert_eq!(panel.desired_height(), 2);
    let lines = panel.lines(80, 8, /*continues_below*/ false, now);
    assert_eq!(lines.len(), 2);
    assert!(line_text(&lines[0]).contains("sleep 1"));
    assert!(line_text(&lines[1]).contains("2 more jobs"));
}

// Covers: whole-second snapshots must not force a redraw every poll.
// Owner: pure layout
#[test]
fn identical_summaries_do_not_mark_the_panel_dirty() {
    let mut panel = ProcessPanel::default();
    let now = Instant::now();
    let processes = vec![summary("aaaaaaaa-1111", "sleep 60", 4)];
    assert!(panel.ingest(processes.clone(), now));
    assert!(!panel.ingest(processes, now));
}

// Covers: a multiline command occupies one rail identity field and omits the id.
// Owner: pure layout
#[test]
fn process_row_uses_first_command_line_without_id() {
    assert_eq!(command_identity("sleep 60\necho still running"), "sleep 60");
    let mut panel = ProcessPanel::default();
    let now = Instant::now();
    panel.ingest(
        vec![summary(
            "550e8400-e29b-41d4-a716-446655440000",
            "sleep 60\necho still running",
            4,
        )],
        now,
    );
    let text = line_text(&panel.lines(80, 8, /*continues_below*/ false, now)[0]);
    assert!(text.contains("sleep 60"));
    assert!(text.contains(activity::PROCESS_GLYPH));
    assert!(!text.contains("550e8400"));
}

// Covers: process activity column styles terminal verdicts by outcome and warns
// on long-quiet live processes.
// Owner: pure unit (process rail styles)
#[test]
fn process_activity_styles_match_state() {
    let exited = |exit_code| LiveProcessSummary {
        exit_code,
        ..with_state(summary("id", "sleep", 4), State::Exited)
    };
    let cases = [
        ("running", summary("id", "sleep", 4), Theme::text()),
        (
            "starting",
            with_state(summary("id", "sleep", 4), State::Starting),
            Theme::text(),
        ),
        ("exit 0", exited(Some(0)), Theme::activity_rail_success()),
        ("exit 2", exited(Some(2)), Theme::activity_rail_error()),
        (
            "exited without code",
            exited(None),
            Theme::activity_rail_error(),
        ),
        (
            "terminated",
            with_state(summary("id", "sleep", 4), State::Terminated),
            Theme::activity_rail_error(),
        ),
        (
            "timed out",
            with_state(summary("id", "sleep", 4), State::TimedOut),
            Theme::activity_rail_error(),
        ),
        (
            "failed to start",
            with_state(summary("id", "sleep", 4), State::FailedToStart),
            Theme::activity_rail_error(),
        ),
    ];
    for (case, process, style) in cases {
        assert_eq!(process_activity(&process).1, style, "{case}");
    }

    let mut warned = summary("id", "sleep", 4);
    warned.quiet_seconds = Some(QUIET_WARN_AFTER);
    assert_eq!(
        process_trailing_style(&warned),
        Theme::activity_rail_warning()
    );
}

// Covers: terminal process rows linger until the deadline, then drop in one update.
// Owner: pure unit (process linger)
#[test]
fn process_linger_keeps_then_drops_around_deadline() {
    let mut panel = ProcessPanel::default();
    let t0 = Instant::now();
    let ok = LiveProcessSummary {
        process_id: "ok".into(),
        command: "true".into(),
        state: State::Exited,
        elapsed_seconds: 3,
        quiet_seconds: None,
        exit_code: Some(0),
    };
    assert!(panel.ingest(vec![ok.clone()], t0));
    assert_eq!(panel.live_count(), 0);
    assert!(panel.is_active());
    let kept = line_text(&panel.lines(80, 8, /*continues_below*/ false, t0)[0]);
    assert!(kept.contains("✓ exit 0"));

    assert!(!panel.ingest(
        vec![ok.clone()],
        t0 + activity::LINGER_OK - Duration::from_millis(1)
    ));
    assert_eq!(
        panel
            .lines(
                80,
                8,
                /*continues_below*/ false,
                t0 + activity::LINGER_OK
            )
            .len(),
        0
    );
    assert!(panel.ingest(vec![ok], t0 + activity::LINGER_OK));
    assert!(!panel.is_active());
}

// Covers: spinner job count ignores lingering rows.
// Owner: pure unit (live_count)
#[test]
fn live_count_excludes_lingering_rows() {
    let mut panel = ProcessPanel::default();
    let now = Instant::now();
    panel.ingest(
        vec![
            summary("live", "sleep", 1),
            LiveProcessSummary {
                process_id: "done".into(),
                command: "true".into(),
                state: State::Exited,
                elapsed_seconds: 2,
                quiet_seconds: None,
                exit_code: Some(0),
            },
        ],
        now,
    );
    assert_eq!(panel.live_count(), 1);
    assert_eq!(panel.desired_height(), 2);
}

// Covers: verdict styles survive the wide-row paint path.
// Owner: pure layout
#[test]
fn process_verdict_styles_paint_on_wide_rows() {
    let _guard = theme::theme_test_lock();
    Theme::apply_committed("one-half-dark");
    let mut panel = ProcessPanel::default();
    let now = Instant::now();
    panel.ingest(
        vec![LiveProcessSummary {
            process_id: "ok".into(),
            command: "true".into(),
            state: State::Exited,
            elapsed_seconds: 3,
            quiet_seconds: None,
            exit_code: Some(0),
        }],
        now,
    );
    let line = &panel.lines(80, 8, /*continues_below*/ false, now)[0];
    assert_eq!(
        activity_span_style(line, "✓ exit 0"),
        Theme::activity_rail().patch(Theme::activity_rail_success())
    );
    Theme::apply_committed("terminal");
}

// Covers: peek hits live and lingering rows, never the overflow summary or
// a point outside the rail.
// Owner: pure unit (process peek hit-test)
#[test]
fn peek_target_hits_rows_and_skips_summary_and_outside() {
    let mut panel = ProcessPanel::default();
    let now = Instant::now();
    panel.ingest(
        vec![
            summary("live-1", "sleep 1", 3),
            summary("live-2", "sleep 2", 2),
            summary("live-3", "sleep 3", 1),
        ],
        now,
    );
    let area = Rect::new(2, 4, 80, 2);
    assert_eq!(
        panel.peek_target_at(area, 3, 4, now),
        Some(ProcessPeekTarget {
            process_id: "live-1".into(),
        })
    );
    assert_eq!(panel.peek_target_at(area, 3, 5, now), None);
    assert_eq!(panel.peek_target_at(area, 1, 4, now), None);
    assert_eq!(
        panel.peek_target_at(Rect::new(2, 4, 80, 0), 3, 4, now),
        None
    );

    let mut linger = ProcessPanel::default();
    linger.ingest(
        vec![LiveProcessSummary {
            process_id: "done".into(),
            command: "true".into(),
            state: State::Exited,
            elapsed_seconds: 3,
            quiet_seconds: None,
            exit_code: Some(0),
        }],
        now,
    );
    assert_eq!(
        linger.peek_target_at(Rect::new(0, 0, 80, 1), 1, 0, now),
        Some(ProcessPeekTarget {
            process_id: "done".into(),
        })
    );
}

// Covers: hover trailing keeps elapsed and names the peek action.
// Owner: pure layout
#[test]
fn hover_trailing_keeps_elapsed() {
    let mut panel = ProcessPanel::default();
    let now = Instant::now();
    panel.ingest(vec![summary("live-1", "sleep 60", 4)], now);
    panel.set_hovered(Some("live-1"));
    let text = line_text(&panel.lines(80, 8, /*continues_below*/ false, now)[0]);
    assert!(text.contains("⏎ peek · 4s"));
    assert_eq!(
        panel.highlighted_row(8, now),
        Some((0, crate::tui::activity::RailRowState::Hovered))
    );
}

// Covers: highlight index must use the painted height, not the rail cap.
// Owner: pure unit (linger rail selection)
#[test]
fn highlight_uses_painted_height_not_cap() {
    let mut panel = ProcessPanel::default();
    let now = Instant::now();
    panel.ingest(
        vec![summary("a", "sleep 1", 1), summary("b", "sleep 2", 2)],
        now,
    );
    panel.set_hovered(Some("a"));
    assert_eq!(panel.lines(80, 1, /*continues_below*/ false, now).len(), 1);
    assert_eq!(panel.highlighted_row(1, now), None);
    assert_eq!(
        panel.highlighted_row(2, now),
        Some((0, crate::tui::activity::RailRowState::Hovered))
    );
}

// Covers: linger expiry must agree between paint and hit-test.
// Owner: pure unit (process peek hit-test clock)
#[test]
fn peek_target_uses_injected_now() {
    let mut panel = ProcessPanel::default();
    let t0 = Instant::now();
    panel.ingest(
        vec![LiveProcessSummary {
            process_id: "done".into(),
            command: "true".into(),
            state: State::Exited,
            elapsed_seconds: 3,
            quiet_seconds: None,
            exit_code: Some(0),
        }],
        t0,
    );
    let area = Rect::new(0, 0, 80, 1);
    assert!(panel.peek_target_at(area, 1, 0, t0).is_some());
    assert_eq!(
        panel.peek_target_at(area, 1, 0, t0 + activity::LINGER_OK),
        None
    );
}
