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
        vec![PlanInventoryItem {
            plan_id: plan.manifest.plan_id,
            access: RecordAccess::Executable,
            created_at_unix_nanos: plan.manifest.created_at_unix_nanos,
            workspace_identity: plan.manifest.workspace_identity.clone(),
            name: plan.manifest.name.clone(),
            step_count: plan.manifest.step_count,
        }]
    );
    assert_eq!(
        store.list_run_inventory().unwrap(),
        vec![RunInventoryItem {
            run_id: run.manifest.run_id,
            access: RecordAccess::Executable,
            created_at_unix_nanos: run.manifest.created_at_unix_nanos,
            workspace_identity: run.manifest.workspace_identity.clone(),
            name: run.manifest.name.clone(),
            lifecycle: RunLifecycle::Planned,
            outcome: None,
            done_steps: 0,
            total_steps: run.manifest.step_count,
        }]
    );
    store.delete_run(run.manifest.run_id).unwrap();
    assert!(store.read_run_inventory(run.manifest.run_id).is_err());
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
