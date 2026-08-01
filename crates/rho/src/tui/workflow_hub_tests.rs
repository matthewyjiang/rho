use std::str::FromStr;

use super::{hub_picker, test_source};
use crate::workflow::{RunId, RunInventoryItem, RunLifecycle, WorkflowOutcome};
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
    assert!(start.value.contains("review") || start.value.contains("workflow.star"));
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
        created_at_unix_nanos,
        workspace_identity: "workspace".into(),
        name: "review".into(),
        lifecycle: RunLifecycle::Completed,
        outcome: Some(WorkflowOutcome::Success),
        done_steps: 1,
        total_steps: 1,
    }
}

// Covers: the hub lists the newest finished run first even when UUID order differs.
// Owner: workflow hub inventory projection.
#[test]
fn hub_picker_orders_runs_by_creation_time() {
    let older = finished_run("ffffffff-ffff-4fff-8fff-ffffffffffff", 1);
    let newer = finished_run("00000000-0000-4000-8000-000000000000", 2);
    let runs = vec![older, newer];
    let picker = hub_picker(&[], &[], &runs);
    let run_values = picker
        .items
        .iter()
        .filter(|item| item.value.starts_with("run:"))
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        run_values,
        vec![
            "run:00000000-0000-4000-8000-000000000000",
            "run:ffffffff-ffff-4fff-8fff-ffffffffffff",
        ]
    );
}

// Covers: legacy runs without a timestamp keep a stable order across hub refreshes.
// Owner: workflow hub inventory projection.
#[test]
fn hub_picker_orders_legacy_zero_timestamps_by_run_id() {
    let larger_id = finished_run("ffffffff-ffff-4fff-8fff-ffffffffffff", 0);
    let smaller_id = finished_run("00000000-0000-4000-8000-000000000000", 0);
    let picker = hub_picker(&[], &[], &[smaller_id, larger_id]);
    let run_values = picker
        .items
        .iter()
        .filter(|item| item.value.starts_with("run:"))
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        run_values,
        vec![
            "run:ffffffff-ffff-4fff-8fff-ffffffffffff",
            "run:00000000-0000-4000-8000-000000000000",
        ]
    );
}
