use std::{collections::BTreeMap, fs::OpenOptions, io::Write};

use super::*;
use crate::workflow::{
    graph_digest,
    test_support::{agent_node, id, state, workflow},
    ArtifactRef, AttemptArtifacts, AttemptNumber, CommandExit, CommandNode, Digest, ExternalOwner,
    NodeCompletion, NodeTerminalState, PlanConsent, RunStateRecord, WorkflowEvent, WorkflowState,
    WorkspaceAccess, EVENT_VERSION, RUN_STATE_VERSION,
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
                state: initial_state(&plan.graph),
            },
        )
        .unwrap()
}

fn initial_state(workflow: &FrozenWorkflow) -> WorkflowState {
    let mut state = state(workflow);
    state.lifecycle = RunLifecycle::Planned;
    state
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
        state: initial_state(&plan.graph),
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
                state: initial_state(&plan.graph),
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

// Covers: a valid journal tail must not authenticate a snapshot with different state.
// Owner: workflow durable store replay boundary.
#[test]
fn rejects_snapshot_state_that_differs_from_its_journal_prefix() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = run(&store, &plan);
    let mut guard = store.lock_run(run.manifest.run_id).unwrap();
    let event = WorkflowEventRecord {
        schema_version: EVENT_VERSION,
        sequence: 1,
        event: WorkflowEvent::CancellationRequested {
            request_id: "00000000-0000-0000-0000-000000000003".into(),
        },
    };
    store.append_event(&mut guard, &event).unwrap();
    let mut snapshot = run.state;
    snapshot.last_event_sequence = 1;
    snapshot.state = derive_snapshot(
        &plan.graph,
        std::slice::from_ref(&event),
        1,
        &store.layout.run_state(run.manifest.run_id),
    )
    .unwrap();
    store.save_state(&guard, &snapshot).unwrap();
    snapshot.state.cancellation_requested = false;
    write_json(&store.layout.run_state(run.manifest.run_id), &snapshot).unwrap();
    drop(guard);

    assert!(matches!(
        store.load_run(run.manifest.run_id),
        Err(WorkflowError::Corrupt { .. })
    ));
}

fn artifact(run: &std::path::Path, name: &str, bytes: &[u8]) -> ArtifactRef {
    use sha2::{Digest as _, Sha256};

    std::fs::write(run.join(name), bytes).unwrap();
    ArtifactRef {
        relative_path: name.to_owned(),
        retained_bytes: bytes.len() as u64,
        observed: ArtifactObservation::Complete {
            observed_bytes: bytes.len() as u64,
        },
        digest: Digest(format!("sha256:{:x}", Sha256::digest(bytes))),
    }
}

fn command_stream_fixture(
    run_directory: &std::path::Path,
    stream_bytes: usize,
    workflow_total: u64,
) -> (FrozenWorkflow, RunStateRecord, Vec<WorkflowEventRecord>) {
    let mut workflow = workflow(vec![agent_node("inspect", &[], WorkspaceAccess::Mutating)]);
    let node = workflow.graph.nodes.get_mut(&id("inspect")).unwrap();
    node.execution = NodeExecution::Command(CommandNode::Direct {
        executable: "/frozen/command".into(),
        arguments: Vec::new(),
        cwd: ".".into(),
        output: None,
    });
    node.max_output_bytes = 4;
    workflow.runtime_limits.retained_output_per_stream_bytes = 4;
    workflow.runtime_limits.retained_output_total_bytes = workflow_total;
    let attempt = AttemptNumber::new(1).unwrap();
    let stdout = artifact(run_directory, "stdout", &vec![b'o'; stream_bytes]);
    let stderr = artifact(run_directory, "stderr", b"eeee");
    let command_outcome = artifact(run_directory, "command.json", b"outcome");
    let completion = NodeCompletion {
        attempt: Some(attempt),
        outcome: NodeTerminalState::Success,
        cancellation_resume: None,
        command_exit: Some(CommandExit::Code { code: 0 }),
        structured_output: None,
        artifacts: AttemptArtifacts {
            stdout: Some(stdout),
            stderr: Some(stderr),
            command_outcome: Some(command_outcome),
            ..AttemptArtifacts::default()
        },
    };
    let events = vec![
        WorkflowEvent::RunLifecycle {
            lifecycle: RunLifecycle::Running,
        },
        WorkflowEvent::NodeReady {
            node: id("inspect"),
        },
        WorkflowEvent::AttemptStarted {
            node: id("inspect"),
            attempt,
            owner: ExternalOwner::Process { pid: 1 },
        },
        WorkflowEvent::NodeFinished {
            node: id("inspect"),
            completion: Box::new(completion),
        },
    ]
    .into_iter()
    .enumerate()
    .map(|(index, event)| WorkflowEventRecord {
        schema_version: EVENT_VERSION,
        sequence: index as u64 + 1,
        event,
    })
    .collect::<Vec<_>>();
    let state = derive_snapshot(
        &workflow,
        &events,
        events.len() as u64,
        &run_directory.join("state.json"),
    )
    .unwrap();
    (
        workflow,
        RunStateRecord {
            schema_version: RUN_STATE_VERSION,
            last_event_sequence: events.len() as u64,
            state,
        },
        events,
    )
}

// Covers: each command stream owns the full per-stream budget, while their sum owns the total.
// Owner: workflow durable output contract.
#[test]
fn validates_two_stream_boundaries_and_workflow_total() {
    let cases = [(4, 8, true), (5, 9, false), (4, 7, false)];
    for (stream_bytes, workflow_total, accepted) in cases {
        let run = tempfile::tempdir().unwrap();
        let (workflow, state, events) =
            command_stream_fixture(run.path(), stream_bytes, workflow_total);
        let root = crate::workflow::secure_fs::SecureDirectory::open(run.path()).unwrap();
        let result = validate_state(
            &workflow,
            &state,
            &events,
            &run.path().join("state.json"),
            &root,
            Path::new(""),
        );
        assert_eq!(
            result.is_ok(),
            accepted,
            "case: {stream_bytes}/{workflow_total}"
        );
    }
}

// Covers: control-file symlinks and broad Unix modes must not reach durable workflow data.
// Owner: workflow durable store filesystem boundary.
#[cfg(unix)]
#[test]
fn rejects_untrusted_control_files() {
    use std::os::unix::fs::{symlink, PermissionsExt as _};

    for target in ["mutation.lock", "events.jsonl", "state.json"] {
        let home = tempfile::tempdir().unwrap();
        let store = WorkflowStore::new(home.path()).unwrap();
        let plan = plan(&store);
        let run = run(&store, &plan);
        let path = store.layout.run(run.manifest.run_id).join(target);
        let outside = home.path().join("outside");
        std::fs::write(&outside, b"outside").unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();
        let result = if target == "mutation.lock" {
            store.lock_run(run.manifest.run_id).map(|_| ())
        } else if target == "events.jsonl" {
            store.read_events(run.manifest.run_id).map(|_| ())
        } else {
            store.load_run(run.manifest.run_id).map(|_| ())
        };
        assert!(result.is_err(), "accepted symlink for {target}");
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
    }

    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let source_plan = plan(&store);
    let digest = source_plan
        .manifest
        .source_digests
        .values()
        .next()
        .unwrap()
        .0
        .strip_prefix("sha256:")
        .unwrap();
    let source = store
        .layout
        .plan_sources(source_plan.manifest.plan_id)
        .join(format!("{digest}.star"));
    let outside = home.path().join("outside-source");
    std::fs::write(&outside, b"WORKFLOW = None").unwrap();
    std::fs::remove_file(&source).unwrap();
    symlink(&outside, &source).unwrap();
    assert!(store.load_plan(source_plan.manifest.plan_id).is_err());

    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = run(&store, &plan);
    let events = store.layout.run_events(run.manifest.run_id);
    std::fs::set_permissions(&events, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.read_events(run.manifest.run_id).is_err());
}

#[cfg(any(unix, windows))]
fn substitute_directory(path: &Path) -> std::io::Result<()> {
    let held = path.with_extension("held");
    std::fs::rename(path, &held)?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&held, path)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&held, path)?;
    Ok(())
}

// Covers: status, run, and mutation must not follow a substituted durable ID directory.
// Owner: workflow durable store filesystem boundary.
#[cfg(any(unix, windows))]
#[test]
fn rejects_substituted_plan_and_run_directories() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = run(&store, &plan);

    if substitute_directory(&store.layout.plan(plan.manifest.plan_id)).is_err() {
        return;
    }
    assert!(store.load_plan(plan.manifest.plan_id).is_err());

    if substitute_directory(&store.layout.run(run.manifest.run_id)).is_err() {
        return;
    }
    assert!(store.load_run(run.manifest.run_id).is_err());
    assert!(store.lock_run(run.manifest.run_id).is_err());
}

// Covers: no durable operation may start below a substituted plans or runs ancestor.
// Owner: workflow durable store filesystem boundary.
#[cfg(any(unix, windows))]
#[test]
fn rejects_substituted_store_ancestors() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = run(&store, &plan);

    if substitute_directory(&store.layout.plans()).is_err() {
        return;
    }
    assert!(store.load_plan(plan.manifest.plan_id).is_err());
    assert!(store
        .resolve_plan(&plan.manifest.plan_id.to_string())
        .is_err());

    if substitute_directory(&store.layout.runs()).is_err() {
        return;
    }
    assert!(store.load_run(run.manifest.run_id).is_err());
    assert!(store.resolve_run(&run.manifest.run_id.to_string()).is_err());
    assert!(store.lock_run(run.manifest.run_id).is_err());
}

// Covers: prefix lookup must enumerate the held workflows root, not its mutable pathname.
// Owner: workflow durable store filesystem boundary.
#[cfg(any(unix, windows))]
#[test]
fn prefix_lookup_ignores_a_substituted_workflows_path() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let held = home.path().join("workflows-held");
    std::fs::rename(store.layout.root(), &held).unwrap();
    let attacker = home.path().join("attacker-workflows");
    let run = RunId::new();
    std::fs::create_dir_all(attacker.join("plans")).unwrap();
    std::fs::create_dir_all(attacker.join("runs").join(run.to_string())).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&attacker, store.layout.root()).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(&attacker, store.layout.root()).is_err() {
        return;
    }

    assert!(store.resolve_run(&run.to_string()).is_err());
}

// Covers: cancellation markers must stay in the held workflow root after its
// pathname is replaced.
// Owner: workflow durable store filesystem boundary.
#[cfg(unix)]
#[test]
fn cancellation_marker_uses_the_held_workflows_root() {
    let home = tempfile::tempdir().unwrap();
    let store = WorkflowStore::new(home.path()).unwrap();
    let plan = plan(&store);
    let run = run(&store, &plan);
    let held = home.path().join("workflows-held");
    std::fs::rename(store.layout.root(), &held).unwrap();
    let attacker = home.path().join("attacker-workflows");
    std::fs::create_dir_all(attacker.join("plans")).unwrap();
    std::fs::create_dir_all(attacker.join("runs").join(run.manifest.run_id.to_string())).unwrap();
    std::os::unix::fs::symlink(&attacker, store.layout.root()).unwrap();

    let request = b"00000000-0000-4000-8000-000000000000";
    assert!(store
        .install_cancellation_request(run.manifest.run_id, request)
        .unwrap());
    assert_eq!(
        store
            .read_cancellation_request(run.manifest.run_id)
            .unwrap(),
        Some(request.to_vec())
    );
    assert!(!attacker
        .join("runs")
        .join(run.manifest.run_id.to_string())
        .join("cancel.request")
        .exists());

    store
        .clear_cancellation_request(run.manifest.run_id)
        .unwrap();
    assert_eq!(
        store
            .read_cancellation_request(run.manifest.run_id)
            .unwrap(),
        None
    );
}
