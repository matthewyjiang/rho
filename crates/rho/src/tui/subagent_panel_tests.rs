use std::time::Duration;

use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::{SubagentPanel, SubagentPointerTarget};
use crate::{
    subagent::{RunState, RunStatus},
    tools::agent::SubagentSnapshot,
};

fn snapshot(id: &str, agent_id: &str, state: RunState, elapsed_seconds: u64) -> SubagentSnapshot {
    SubagentSnapshot {
        prior_notices: Vec::new(),
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

// Covers: spinner agent count ignores finished, undelivered rows.
// Owner: pure unit (subagent count)
#[test]
fn count_excludes_finished_rows() {
    let mut panel = SubagentPanel::default();
    panel.ingest(vec![
        snapshot("live01", "worker", RunState::Running, 2),
        snapshot("done01", "explorer", RunState::Running, 3),
    ]);
    panel.ingest(vec![
        snapshot("live01", "worker", RunState::Running, 2),
        snapshot("done01", "explorer", RunState::Error, 3),
    ]);
    assert_eq!(panel.count(), 1);
    assert_eq!(panel.desired_height(), 2);
    assert!(panel.candidates().iter().all(|c| c.run_id == "live01"));
}

// Covers: overflow rows collapse into one summary row that opens the attach picker.
// Owner: pure unit (overflow pointer)
#[test]
fn subagent_overflow_summary_opens_attach_picker() {
    let mut panel = SubagentPanel::default();
    panel.ingest(vec![
        snapshot("aa0001", "worker", RunState::Running, 1),
        snapshot("aa0002", "explorer", RunState::Running, 1),
        snapshot("aa0003", "reviewer", RunState::Running, 1),
    ]);
    assert_eq!(panel.lines(80, 8, "attach", false).len(), 2);

    let area = Rect::new(0, 0, 80, 2);
    assert_eq!(
        panel.attach_target_at(area, 1, 1),
        Some(SubagentPointerTarget::OpenAttachPicker)
    );
    assert!(matches!(
        panel.attach_target_at(area, 1, 0),
        Some(SubagentPointerTarget::Run(_))
    ));
}

// Covers: finished, undelivered rows occupy height but are not attachable.
// Owner: pure unit (attach gating)
#[test]
fn finished_subagent_rows_are_not_clickable() {
    let mut panel = SubagentPanel::default();
    panel.ingest(vec![snapshot("aa0001", "worker", RunState::Running, 4)]);
    panel.ingest(vec![snapshot("aa0001", "worker", RunState::Ok, 4)]);
    let area = Rect::new(0, 0, 80, 1);
    assert_eq!(panel.attach_target_at(area, 1, 0), None);
    assert!(panel.candidates().is_empty());
}
