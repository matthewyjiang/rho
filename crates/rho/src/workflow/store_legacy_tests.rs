use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::plan_relative;
use super::*;
use crate::workflow::{PlanInventoryItem, RecordAccess, RunInventoryItem, WorkflowStore};

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
fn write_legacy_records(store: &WorkflowStore, lifecycle: &str) {
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
    // Frozen from the single-graph NodeCompletion/AttemptArtifacts format at 2ea7ad26.
    let mut state: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/legacy_run_state.json")).unwrap();
    state["state"]["lifecycle"] = json!(lifecycle);
    write(store, run_relative(run_id, Path::new("state.json")), &state);
}

// Covers: plans and runs saved before scoped programs stay listed and readable
// after upgrade, but cannot be executed against the new runtime.
// Owner: workflow durable store (legacy read-only records).
#[test]
fn legacy_records_are_listed_and_read_only() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    write_legacy_records(&store, "completed");
    let plan_id: PlanId = PLAN_ID.parse().unwrap();
    let run_id: RunId = RUN_ID.parse().unwrap();

    assert_eq!(
        store.list_plan_inventory().unwrap(),
        vec![PlanInventoryItem {
            plan_id,
            access: RecordAccess::ReadOnly,
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
            access: RecordAccess::ReadOnly,
            created_at_unix_nanos: 8,
            workspace_identity: "workspace-id".to_owned(),
            name: "review".to_owned(),
            lifecycle: RunLifecycle::Completed,
            outcome: Some(WorkflowOutcome::Success),
            done_steps: 1,
            total_steps: 1,
        }]
    );

    let RunRecord::Legacy(legacy) = store.load_run_record(run_id).unwrap() else {
        panic!("expected a read-only legacy record");
    };
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/legacy_run_state.json")).unwrap();
    assert_eq!(serde_json::to_value(&legacy.state).unwrap(), expected);

    for error in [
        store.load_plan(plan_id).unwrap_err(),
        store.load_run(run_id).unwrap_err(),
    ] {
        assert!(
            matches!(error, WorkflowError::LegacyRecord { .. }),
            "{error:?}"
        );
    }

    // A legacy journal may end mid-write. Refusing a writer must not repair it.
    let partial = b"{\"schema_version\":";
    store
        .root
        .write_file(&run_relative(run_id, Path::new("events.jsonl")), partial)
        .unwrap();
    let error = match store.lock_run(run_id) {
        Ok(_) => panic!("legacy record accepted a writer"),
        Err(error) => error,
    };
    assert!(matches!(error, WorkflowError::LegacyRecord { .. }));
    assert!(matches!(
        store
            .install_cancellation_request(run_id, b"request")
            .unwrap_err(),
        WorkflowError::LegacyRecord { .. }
    ));
    assert!(matches!(
        store.clear_cancellation_request(run_id).unwrap_err(),
        WorkflowError::LegacyRecord { .. }
    ));
    assert_eq!(
        std::fs::read(store.layout.run_events(run_id)).unwrap(),
        partial
    );
    assert!(!store.layout.run(run_id).join("cancel.request").exists());

    store.delete_run(run_id).unwrap();
    store.delete_plan(plan_id).unwrap();
    assert_eq!(store.list_run_inventory().unwrap(), vec![]);
    assert_eq!(store.list_plan_inventory().unwrap(), vec![]);
}

// Covers: a legacy run left running by an older release cannot be cancelled or
// resumed, so delete must clear it once no process holds its writer lock.
// Owner: workflow durable store (legacy read-only records).
#[test]
fn abandoned_live_legacy_run_is_deletable_only_without_a_writer() {
    use fs2::FileExt as _;

    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    write_legacy_records(&store, "running");
    let run_id: RunId = RUN_ID.parse().unwrap();

    let writer = std::fs::File::open(store.layout.run_lock(run_id)).unwrap();
    writer.try_lock_exclusive().unwrap();
    assert!(matches!(
        store.delete_run(run_id).unwrap_err(),
        WorkflowError::Corrupt { .. }
    ));
    writer.unlock().unwrap();

    store.delete_run(run_id).unwrap();
    assert_eq!(store.list_run_inventory().unwrap(), vec![]);
}
