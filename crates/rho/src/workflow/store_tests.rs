use std::{collections::BTreeMap, fs::OpenOptions, io::Write};

use super::*;
use crate::workflow::{
    test_support::{agent_node, state, workflow},
    PlanConsent, RunStateRecord, WorkflowEvent, WorkspaceAccess, EVENT_VERSION, RUN_STATE_VERSION,
};

fn plan(store: &WorkflowStore) -> StoredPlan {
    let workflow = workflow(vec![agent_node("inspect", &[], WorkspaceAccess::Mutating)]);
    store
        .create_plan(
            &workflow,
            "workspace-id".to_owned(),
            &BTreeMap::from([("//workflow.star".to_owned(), "WORKFLOW = None".to_owned())]),
        )
        .unwrap()
}

// Covers: plan deletion or source edits must not break run status and resume data.
// Owner: workflow durable store.
#[test]
fn run_keeps_an_independent_frozen_graph() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let state = RunStateRecord {
        schema_version: RUN_STATE_VERSION,
        last_event_sequence: 0,
        state: state(&plan.graph),
    };
    let run = store
        .create_run(
            &plan,
            PlanConsent {
                graph_digest: plan.manifest.graph_digest.clone(),
                confirmed: true,
            },
            state,
        )
        .unwrap();
    std::fs::remove_dir_all(store.layout.plan(plan.manifest.plan_id)).unwrap();
    assert_eq!(store.load_run(run.manifest.run_id).unwrap(), run);
}

// Covers: a crash-truncated journal tail may hide all earlier durable transitions.
// Owner: workflow durable store.
#[test]
fn ignores_only_one_truncated_final_event() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = store
        .create_run(
            &plan,
            PlanConsent {
                graph_digest: plan.manifest.graph_digest.clone(),
                confirmed: true,
            },
            RunStateRecord {
                schema_version: RUN_STATE_VERSION,
                last_event_sequence: 0,
                state: state(&plan.graph),
            },
        )
        .unwrap();
    let event = WorkflowEventRecord {
        schema_version: EVENT_VERSION,
        sequence: 1,
        event: WorkflowEvent::CancellationRequested,
    };
    let mut guard = store.lock_run(run.manifest.run_id).unwrap();
    store.append_event(&mut guard, &event).unwrap();
    let mut file = OpenOptions::new()
        .append(true)
        .open(store.layout.run_events(run.manifest.run_id))
        .unwrap();
    file.write_all(b"{\"schema_version\":").unwrap();
    assert_eq!(store.read_events(run.manifest.run_id).unwrap(), vec![event]);
}
