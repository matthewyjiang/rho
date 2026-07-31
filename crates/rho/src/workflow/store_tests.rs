use std::{collections::BTreeMap, fs::OpenOptions, io::Write};

use super::*;
use crate::workflow::{
    graph_digest,
    test_support::{agent_node, id, state, workflow},
    Digest, PlanConsent, RunStateRecord, WorkflowEvent, WorkspaceAccess, EVENT_VERSION,
    RUN_STATE_VERSION,
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

fn run(store: &WorkflowStore, plan: &StoredPlan) -> StoredRun {
    store
        .create_run(
            plan,
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

// Covers: a crash-truncated journal tail could block resume and every later append.
// Owner: workflow durable store.
#[test]
fn repairs_one_truncated_final_event_and_appends_from_the_prefix() {
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
        event: WorkflowEvent::CancellationRequested {
            request_id: "00000000-0000-0000-0000-000000000001".into(),
        },
    };
    let mut guard = store.lock_run(run.manifest.run_id).unwrap();
    store.append_event(&mut guard, &event).unwrap();
    drop(guard);
    let mut file = OpenOptions::new()
        .append(true)
        .open(store.layout.run_events(run.manifest.run_id))
        .unwrap();
    file.write_all(b"{\"schema_version\":").unwrap();
    file.sync_all().unwrap();
    drop(file);

    let second = WorkflowEventRecord {
        schema_version: EVENT_VERSION,
        sequence: 2,
        event: WorkflowEvent::CancellationAcknowledged {
            request_id: "00000000-0000-0000-0000-000000000001".into(),
        },
    };
    let mut guard = store.lock_run(run.manifest.run_id).unwrap();
    store.append_event(&mut guard, &second).unwrap();
    drop(guard);

    assert_eq!(
        store.read_events(run.manifest.run_id).unwrap(),
        vec![event, second]
    );
    let bytes = std::fs::read(store.layout.run_events(run.manifest.run_id)).unwrap();
    assert!(bytes.ends_with(b"\n"));
}

// Covers: accepting a journal that starts after sequence one would hide state transitions.
// Owner: workflow durable store.
#[test]
fn rejects_a_journal_whose_first_sequence_is_not_one() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = run(&store, &plan);
    let record = WorkflowEventRecord {
        schema_version: EVENT_VERSION,
        sequence: 2,
        event: WorkflowEvent::CancellationRequested {
            request_id: "00000000-0000-0000-0000-000000000002".into(),
        },
    };
    std::fs::write(
        store.layout.run_events(run.manifest.run_id),
        format!("{}\n", serde_json::to_string(&record).unwrap()),
    )
    .unwrap();

    assert!(matches!(
        store.read_events(run.manifest.run_id),
        Err(WorkflowError::Corrupt { .. })
    ));
    assert!(matches!(
        store.lock_run(run.manifest.run_id),
        Err(WorkflowError::Corrupt { .. })
    ));
}

// Covers: treating a malformed complete record as a torn tail would erase corruption on resume.
// Owner: workflow durable store.
#[test]
fn rejects_malformed_complete_journal_records() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = run(&store, &plan);
    std::fs::write(
        store.layout.run_events(run.manifest.run_id),
        b"{not-json}\n",
    )
    .unwrap();

    assert!(matches!(
        store.read_events(run.manifest.run_id),
        Err(WorkflowError::Corrupt { .. })
    ));
    assert!(matches!(
        store.lock_run(run.manifest.run_id),
        Err(WorkflowError::Corrupt { .. })
    ));
}

#[derive(Clone, Copy, Debug)]
enum DurableCorruption {
    PlanId,
    PlanGraphDigest,
    PlanWorkspace,
    SourceMetadata,
    SourceBlob,
    InvalidGraph,
    RunId,
    RunGraphDigest,
    RunConsent,
    RunWorkspace,
    NodeKeys,
    SnapshotSequence,
}

// Covers: tampered duplicate durable fields could make status and resume trust different plans.
// Owner: workflow durable store boundary.
#[test]
fn rejects_tampered_durable_records_at_load() {
    for corruption in [
        DurableCorruption::PlanId,
        DurableCorruption::PlanGraphDigest,
        DurableCorruption::PlanWorkspace,
        DurableCorruption::SourceMetadata,
        DurableCorruption::SourceBlob,
        DurableCorruption::InvalidGraph,
        DurableCorruption::RunId,
        DurableCorruption::RunGraphDigest,
        DurableCorruption::RunConsent,
        DurableCorruption::RunWorkspace,
        DurableCorruption::NodeKeys,
        DurableCorruption::SnapshotSequence,
    ] {
        let home = tempfile::tempdir().unwrap();
        let store = WorkflowStore::new(home.path()).unwrap();
        let plan = plan(&store);
        let run = run(&store, &plan);
        let result = match corruption {
            DurableCorruption::PlanId => {
                let mut manifest = plan.manifest.clone();
                manifest.plan_id = PlanId::new();
                write_json(
                    &store.layout.plan_manifest(plan.manifest.plan_id),
                    &manifest,
                )
                .unwrap();
                store.load_plan(plan.manifest.plan_id).map(|_| ())
            }
            DurableCorruption::PlanGraphDigest => {
                let mut manifest = plan.manifest.clone();
                manifest.graph_digest = Digest("sha256:00".to_owned());
                write_json(
                    &store.layout.plan_manifest(plan.manifest.plan_id),
                    &manifest,
                )
                .unwrap();
                store.load_plan(plan.manifest.plan_id).map(|_| ())
            }
            DurableCorruption::PlanWorkspace => {
                let mut manifest = plan.manifest.clone();
                manifest.workspace_identity.clear();
                write_json(
                    &store.layout.plan_manifest(plan.manifest.plan_id),
                    &manifest,
                )
                .unwrap();
                store.load_plan(plan.manifest.plan_id).map(|_| ())
            }
            DurableCorruption::SourceMetadata => {
                let mut graph = plan.graph.clone();
                graph
                    .sources
                    .modules
                    .get_mut("//workflow.star")
                    .unwrap()
                    .bytes += 1;
                graph.graph_digest = graph_digest(&graph).unwrap();
                let mut manifest = plan.manifest.clone();
                manifest.graph_digest = graph.graph_digest.clone();
                write_json(&store.layout.plan_graph(plan.manifest.plan_id), &graph).unwrap();
                write_json(
                    &store.layout.plan_manifest(plan.manifest.plan_id),
                    &manifest,
                )
                .unwrap();
                store.load_plan(plan.manifest.plan_id).map(|_| ())
            }
            DurableCorruption::SourceBlob => {
                let digest = plan
                    .manifest
                    .source_digests
                    .values()
                    .next()
                    .unwrap()
                    .0
                    .strip_prefix("sha256:")
                    .unwrap();
                std::fs::write(
                    store
                        .layout
                        .plan_sources(plan.manifest.plan_id)
                        .join(format!("{digest}.star")),
                    b"tampered",
                )
                .unwrap();
                store.load_plan(plan.manifest.plan_id).map(|_| ())
            }
            DurableCorruption::InvalidGraph => {
                let mut graph = plan.graph.clone();
                graph.graph.nodes.get_mut(&id("inspect")).unwrap().needs = vec![id("inspect")];
                graph.graph_digest = graph_digest(&graph).unwrap();
                let mut manifest = plan.manifest.clone();
                manifest.graph_digest = graph.graph_digest.clone();
                write_json(&store.layout.plan_graph(plan.manifest.plan_id), &graph).unwrap();
                write_json(
                    &store.layout.plan_manifest(plan.manifest.plan_id),
                    &manifest,
                )
                .unwrap();
                store.load_plan(plan.manifest.plan_id).map(|_| ())
            }
            DurableCorruption::RunId => {
                let mut manifest = run.manifest.clone();
                manifest.run_id = RunId::new();
                write_json(&store.layout.run_manifest(run.manifest.run_id), &manifest).unwrap();
                store.load_run(run.manifest.run_id).map(|_| ())
            }
            DurableCorruption::RunGraphDigest => {
                let mut manifest = run.manifest.clone();
                manifest.graph_digest = Digest("sha256:00".to_owned());
                write_json(&store.layout.run_manifest(run.manifest.run_id), &manifest).unwrap();
                store.load_run(run.manifest.run_id).map(|_| ())
            }
            DurableCorruption::RunConsent => {
                let mut manifest = run.manifest.clone();
                manifest.consent.confirmed = false;
                write_json(&store.layout.run_manifest(run.manifest.run_id), &manifest).unwrap();
                store.load_run(run.manifest.run_id).map(|_| ())
            }
            DurableCorruption::RunWorkspace => {
                let mut manifest = run.manifest.clone();
                manifest.workspace_identity.clear();
                write_json(&store.layout.run_manifest(run.manifest.run_id), &manifest).unwrap();
                store.load_run(run.manifest.run_id).map(|_| ())
            }
            DurableCorruption::NodeKeys => {
                let mut state = run.state.clone();
                let node_state = state.state.nodes.remove(&id("inspect")).unwrap();
                state.state.nodes.insert(id("other"), node_state);
                write_json(&store.layout.run_state(run.manifest.run_id), &state).unwrap();
                store.load_run(run.manifest.run_id).map(|_| ())
            }
            DurableCorruption::SnapshotSequence => {
                let mut state = run.state.clone();
                state.last_event_sequence = 1;
                write_json(&store.layout.run_state(run.manifest.run_id), &state).unwrap();
                store.load_run(run.manifest.run_id).map(|_| ())
            }
        };
        assert!(result.is_err(), "corruption was accepted: {corruption:?}");
    }
}
