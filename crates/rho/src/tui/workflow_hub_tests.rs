use std::str::FromStr;

use super::{hub_picker, test_source};
use crate::workflow::{RecordAccess, RunId, RunInventoryItem, RunLifecycle, WorkflowOutcome};
use pretty_assertions::assert_eq;

// Covers: a discovered source is startable from the hub (action identity, not chrome copy).
// Owner: workflow hub inventory projection.
#[test]
fn hub_picker_exposes_startable_source_action() {
    let sources = vec![test_source("review", ".rho/workflows/review/workflow.star")];
    let picker = hub_picker(&sources, &[], &[]);
    assert!(picker.is_overlay());
    let start = picker
        .items
        .iter()
        .find(|item| item.value.starts_with("source:"))
        .expect("start row");
    assert_eq!(start.selection_verb, Some("start"));
}

// Covers: empty inventory keeps a non-startable placeholder row with a stable action id.
// Owner: workflow hub inventory projection.
#[test]
fn hub_picker_marks_empty_start_when_no_sources() {
    let picker = hub_picker(&[], &[], &[]);
    assert_eq!(picker.items[0].value, "noop:empty_sources");
    // Placeholder closes the hub; it must not start a run.
    assert_ne!(picker.items[0].selection_verb, Some("start"));
}

fn finished_run(id: &str, created_at_unix_nanos: u64) -> RunInventoryItem {
    RunInventoryItem {
        run_id: RunId::from_str(id).unwrap(),
        access: RecordAccess::Executable,
        created_at_unix_nanos,
        workspace_identity: "workspace".into(),
        name: "review".into(),
        lifecycle: RunLifecycle::Completed,
        outcome: Some(WorkflowOutcome::Success),
        done_steps: 1,
        total_steps: 1,
    }
}

// Covers: the hub lists the newest finished run first even when UUID order
// differs, and legacy runs without a timestamp keep a stable order across refreshes.
// Owner: workflow hub inventory projection.
#[test]
fn hub_picker_orders_runs_by_creation_time_then_run_id() {
    const LOW: &str = "00000000-0000-4000-8000-000000000000";
    const HIGH: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    for (case, runs, expected) in [
        (
            "newest first",
            vec![finished_run(HIGH, 1), finished_run(LOW, 2)],
            [LOW, HIGH],
        ),
        (
            "legacy zero timestamps by run id",
            vec![finished_run(LOW, 0), finished_run(HIGH, 0)],
            [HIGH, LOW],
        ),
    ] {
        let picker = hub_picker(&[], &[], &runs);
        let run_values = picker
            .items
            .iter()
            .filter_map(|item| item.value.strip_prefix("run:"))
            .collect::<Vec<_>>();
        assert_eq!(run_values, expected, "{case}");
    }
}
