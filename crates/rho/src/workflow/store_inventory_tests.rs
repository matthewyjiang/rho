use super::*;
use pretty_assertions::assert_eq;

// Covers: hub plan/run cleanup removes only the targeted store entry.
// Owner: workflow durable store.
#[test]
fn deletes_plans_and_runs() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let first = plan(&store);
    let second = plan(&store);
    let run = run(&store, &first);
    store.delete_plan(first.manifest.plan_id).unwrap();
    assert!(store.load_plan(first.manifest.plan_id).is_err());
    assert!(store.load_plan(second.manifest.plan_id).is_ok());
    assert_eq!(
        store.load_run(run.manifest.run_id).unwrap().manifest.run_id,
        run.manifest.run_id
    );
    store.delete_run(run.manifest.run_id).unwrap();
    assert!(store.load_run(run.manifest.run_id).is_err());
}

// Covers: deletion cannot remove the tree while an owner holds the writer lock.
// Owner: workflow durable store.
#[test]
fn delete_run_fails_while_writer_lock_is_held() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = run(&store, &plan);
    let guard = store.lock_run(run.manifest.run_id).unwrap();
    assert!(matches!(
        store.delete_run(run.manifest.run_id).unwrap_err(),
        WorkflowError::Corrupt { .. }
    ));
    drop(guard);
    assert_eq!(
        store.load_run(run.manifest.run_id).unwrap().manifest.run_id,
        run.manifest.run_id
    );
}

// Covers: crashed live owners are refused by deletion, with an actionable error.
// Owner: workflow durable store.
#[test]
fn delete_run_refuses_live_lifecycle() {
    for lifecycle in [RunLifecycle::Running, RunLifecycle::Cancelling] {
        let home = tempfile::tempdir().unwrap();
        let store = WorkflowStore::new(home.path()).unwrap();
        let plan = plan(&store);
        let run = run(&store, &plan);
        let id = run.manifest.run_id;
        let mut record = run.state.clone();
        record.state.lifecycle = lifecycle;
        write_json(&store.layout.run_state(id), &record).unwrap();
        let err = store.delete_run(id).unwrap_err();
        assert!(
            matches!(err, WorkflowError::LiveRun { id: actual_id, lifecycle: actual_lifecycle } if (actual_id, actual_lifecycle) == (id, lifecycle))
        );
        assert_eq!(
            err.to_string(),
            format!(
                "run {id} is still {}, stop it before deleting",
                lifecycle.as_str()
            )
        );
        assert_eq!(store.read_run_lifecycle(id).unwrap(), lifecycle);
    }
}

// Covers: watch polling reads revision without journal replay.
// Owner: workflow durable store.
#[test]
fn read_run_revision_skips_journal_replay() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = run(&store, &plan);
    std::fs::write(store.layout.run_events(run.manifest.run_id), b"{not-json\n").unwrap();
    assert!(store.load_run(run.manifest.run_id).is_err());
    assert_eq!(
        store.read_run_revision(run.manifest.run_id).unwrap(),
        run.state.state.revision
    );
    assert_eq!(
        store.read_run_lifecycle(run.manifest.run_id).unwrap(),
        RunLifecycle::Planned
    );
}

// Covers: hub inventory and deletion do not require graph/journal validation.
// Owner: workflow durable store.
#[test]
fn plan_and_run_inventory_skips_journal_replay() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = run(&store, &plan);
    std::fs::write(store.layout.run_events(run.manifest.run_id), b"{not-json\n").unwrap();
    assert!(store.load_run(run.manifest.run_id).is_err());
    std::fs::remove_file(store.layout.plan_graph(plan.manifest.plan_id)).unwrap();
    std::fs::remove_file(store.layout.run_graph(run.manifest.run_id)).unwrap();
    assert_eq!(
        store.list_plan_inventory().unwrap(),
        PlanInventory {
            items: vec![PlanInventoryItem {
                plan_id: plan.manifest.plan_id,
                created_at_unix_nanos: plan.manifest.created_at_unix_nanos,
                workspace_identity: plan.manifest.workspace_identity.clone(),
                name: plan.manifest.name.clone(),
                step_count: plan.manifest.step_count,
            }],
            unsupported: Vec::new(),
        }
    );
    assert_eq!(
        store.list_run_inventory().unwrap(),
        RunInventory {
            items: vec![RunInventoryItem {
                run_id: run.manifest.run_id,
                created_at_unix_nanos: run.manifest.created_at_unix_nanos,
                workspace_identity: run.manifest.workspace_identity.clone(),
                name: run.manifest.name.clone(),
                lifecycle: RunLifecycle::Planned,
                outcome: None,
                done_steps: 0,
                total_steps: run.manifest.step_count,
            }],
            unsupported: Vec::new(),
        }
    );
    store.delete_run(run.manifest.run_id).unwrap();
    assert_eq!(store.list_run_inventory().unwrap(), RunInventory::default());
}

// Covers: an unreadable UUID entry must not vanish into an empty hub.
// Owner: workflow durable store.
#[test]
fn list_plan_inventory_fails_closed_on_corrupt_uuid_entry() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    std::fs::write(store.layout.plan_manifest(plan.manifest.plan_id), b"{").unwrap();
    assert!(store.list_plan_inventory().is_err());
}

// Covers: plans and runs saved before scoped programs (version 1 manifests)
// fail to open with an error naming the record instead of vanishing, stay
// listed as unsupported rows, and can still be deleted.
// Owner: workflow durable store generation policy.
#[test]
fn version_1_records_fail_to_open_but_stay_listed_and_deletable() {
    const PLAN_ID: &str = "018f0000-0000-7000-8000-000000000001";
    const RUN_ID: &str = "018f0000-0000-7000-8000-000000000002";
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan_id: PlanId = PLAN_ID.parse().unwrap();
    let run_id: RunId = RUN_ID.parse().unwrap();
    for (relative, manifest) in [
        (
            plan_relative(plan_id, Path::new("manifest.json")),
            serde_json::json!({
                "schema_version": 1, "plan_id": PLAN_ID, "created_at_unix_nanos": 7,
                "graph_digest": "sha256:old", "workspace_identity": "workspace-id",
                "source_digests": {}, "name": "review", "step_count": 1,
            }),
        ),
        (
            run_relative(run_id, Path::new("manifest.json")),
            serde_json::json!({
                "schema_version": 1, "run_id": RUN_ID, "created_at_unix_nanos": 8,
                "plan_id": PLAN_ID, "graph_digest": "sha256:old",
                "workspace_identity": "workspace-id",
                "consent": {"graph_digest": "sha256:old", "confirmed": true},
                "name": "review", "step_count": 1,
            }),
        ),
    ] {
        store
            .root
            .ensure_directory(relative.parent().unwrap())
            .unwrap();
        write_json_beneath(&store.root, &relative, &manifest).unwrap();
    }
    store
        .root
        .write_file(&run_relative(run_id, Path::new("mutation.lock")), b"")
        .unwrap();

    for (error, kind, id) in [
        (store.load_plan(plan_id).unwrap_err(), "plan", PLAN_ID),
        (store.load_run(run_id).unwrap_err(), "run", RUN_ID),
        (
            store.lock_run(run_id).err().expect("writer refused"),
            "run",
            RUN_ID,
        ),
        (
            store
                .install_cancellation_request(run_id, b"request")
                .unwrap_err(),
            "run",
            RUN_ID,
        ),
    ] {
        assert!(
            matches!(
                &error,
                WorkflowError::UnsupportedRecord { kind: actual_kind, id: actual_id }
                    if (*actual_kind, actual_id.as_str()) == (kind, id)
            ),
            "{error:?}"
        );
    }

    assert_eq!(
        store.list_plan_inventory().unwrap(),
        PlanInventory {
            items: Vec::new(),
            unsupported: vec![UnsupportedRecord {
                id: plan_id,
                created_at_unix_nanos: 7,
                workspace_identity: "workspace-id".to_owned(),
                name: "review".to_owned(),
            }],
        }
    );
    assert_eq!(
        store.list_run_inventory().unwrap(),
        RunInventory {
            items: Vec::new(),
            unsupported: vec![UnsupportedRecord {
                id: run_id,
                created_at_unix_nanos: 8,
                workspace_identity: "workspace-id".to_owned(),
                name: "review".to_owned(),
            }],
        }
    );

    store.delete_run(run_id).unwrap();
    store.delete_plan(plan_id).unwrap();
    assert_eq!(store.list_run_inventory().unwrap(), RunInventory::default());
    assert_eq!(
        store.list_plan_inventory().unwrap(),
        PlanInventory::default()
    );
}
