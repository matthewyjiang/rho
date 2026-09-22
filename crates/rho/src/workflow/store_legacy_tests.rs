use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::workflow::{NodeTerminalState, PlanInventoryItem, RunInventoryItem, WorkflowStore};

const PLAN_ID: &str = "018f0000-0000-7000-8000-000000000001";
const RUN_ID: &str = "018f0000-0000-7000-8000-000000000002";

fn write(store: &WorkflowStore, relative: PathBuf, value: &serde_json::Value) {
    store
        .root
        .ensure_directory(relative.parent().unwrap())
        .unwrap();
    super::super::write_json_beneath(&store.root, &relative, value).unwrap();
}

/// Writes a plan and a completed run in the version 1 single-graph layout.
fn write_legacy_records(store: &WorkflowStore) {
    let plan_id: PlanId = PLAN_ID.parse().unwrap();
    let run_id: RunId = RUN_ID.parse().unwrap();
    write(
        store,
        plan_relative(plan_id, Path::new("manifest.json")),
        &json!({
            "schema_version": 1,
            "plan_id": PLAN_ID,
            "created_at_unix_nanos": 7,
            "graph_digest": "sha256:legacy",
            "workspace_identity": "workspace-id",
            "source_digests": {},
            "name": "review",
            "step_count": 1,
        }),
    );
    write(
        store,
        run_relative(run_id, Path::new("manifest.json")),
        &json!({
            "schema_version": 1,
            "run_id": RUN_ID,
            "created_at_unix_nanos": 8,
            "plan_id": PLAN_ID,
            "graph_digest": "sha256:legacy",
            "workspace_identity": "workspace-id",
            "consent": {"graph_digest": "sha256:legacy", "confirmed": true},
            "name": "review",
            "step_count": 1,
        }),
    );
    for file in ["events.jsonl", "mutation.lock"] {
        store
            .root
            .write_file(&run_relative(run_id, Path::new(file)), b"")
            .unwrap();
    }
    write(
        store,
        run_relative(run_id, Path::new("graph.json")),
        &json!({"schema_version": 2, "graph_digest": "sha256:legacy"}),
    );
    write(
        store,
        run_relative(run_id, Path::new("state.json")),
        &json!({
            "schema_version": 2,
            "last_event_sequence": 4,
            "state": {
                "revision": 4,
                "lifecycle": "completed",
                "outcome": "success",
                "cancellation_requested": false,
                "nodes": {"inspect": {"state": "terminal", "outcome": "success"}},
                "command_exits": {},
                "outputs": {"inspect": {"summary": "ok"}},
                "completions": {},
            },
        }),
    );
}

// Covers: plans and runs saved before scoped programs stay listed and readable
// after upgrade, but cannot be executed against the new runtime.
// Owner: workflow durable store (legacy read-only records).
#[test]
fn legacy_records_are_listed_and_read_only() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    write_legacy_records(&store);
    let plan_id: PlanId = PLAN_ID.parse().unwrap();
    let run_id: RunId = RUN_ID.parse().unwrap();

    assert_eq!(
        store.list_plan_inventory().unwrap(),
        vec![PlanInventoryItem {
            plan_id,
            created_at_unix_nanos: 7,
            workspace_identity: "workspace-id".to_owned(),
            name: "review".to_owned(),
            step_count: 1,
        }]
    );
    assert_eq!(
        store.list_run_inventory().unwrap(),
        vec![RunInventoryItem {
            run_id,
            created_at_unix_nanos: 8,
            workspace_identity: "workspace-id".to_owned(),
            name: "review".to_owned(),
            lifecycle: RunLifecycle::Completed,
            outcome: Some(WorkflowOutcome::Success),
            done_steps: 1,
            total_steps: 1,
        }]
    );

    let legacy = store.load_legacy_run(run_id).unwrap();
    let inspect = NodeId::new("inspect").unwrap();
    assert_eq!(
        legacy.state.state,
        LegacyWorkflowState {
            revision: 4,
            lifecycle: RunLifecycle::Completed,
            outcome: Some(WorkflowOutcome::Success),
            cancellation_requested: false,
            nodes: BTreeMap::from([(
                inspect.clone(),
                NodeState::Terminal {
                    outcome: NodeTerminalState::Success
                }
            )]),
            command_exits: BTreeMap::new(),
            outputs: BTreeMap::from([(
                inspect,
                WorkflowValue::from_json(json!({"summary": "ok"})).unwrap()
            )]),
            completions: BTreeMap::new(),
        }
    );

    for error in [
        store.load_plan(plan_id).unwrap_err(),
        store.load_run(run_id).unwrap_err(),
    ] {
        assert!(
            matches!(error, WorkflowError::LegacyRecord { .. }),
            "{error:?}"
        );
    }

    store.delete_run(run_id).unwrap();
    store.delete_plan(plan_id).unwrap();
    assert_eq!(store.list_run_inventory().unwrap(), vec![]);
    assert_eq!(store.list_plan_inventory().unwrap(), vec![]);
}
