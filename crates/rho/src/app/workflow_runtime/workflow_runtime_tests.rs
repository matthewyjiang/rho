use std::{
    collections::BTreeMap,
    io::Write as _,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use crate::workflow::*;

use super::*;

#[derive(Default)]
struct SuccessfulExecutor;

impl WorkflowNodeExecutor for SuccessfulExecutor {
    fn execute<'a>(&'a self, _request: NodeExecutionRequest) -> WorkflowExecutionFuture<'a> {
        Box::pin(async { Ok(NodeExecutionResult::terminal(NodeTerminalState::Success)) })
    }
}

struct CountingExecutor(AtomicUsize);

impl WorkflowNodeExecutor for CountingExecutor {
    fn execute<'a>(&'a self, _request: NodeExecutionRequest) -> WorkflowExecutionFuture<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(NodeExecutionResult::terminal(NodeTerminalState::Success)) })
    }
}

struct CancellationExecutor {
    started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl WorkflowNodeExecutor for CancellationExecutor {
    fn execute<'a>(&'a self, request: NodeExecutionRequest) -> WorkflowExecutionFuture<'a> {
        Box::pin(async move {
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            request.cancellation.cancelled().await;
            Ok(NodeExecutionResult::terminal(
                NodeTerminalState::Cancellation,
            ))
        })
    }
}

fn node_id(value: &str) -> NodeId {
    NodeId::new(value).unwrap()
}

fn test_workflow() -> FrozenWorkflow {
    let node = Node {
        id: node_id("inspect"),
        display_name: "inspect".into(),
        needs: Vec::new(),
        condition: None,
        execution: NodeExecution::Agent(AgentNode {
            agent: "reviewer".into(),
            prompt: Template(vec![TemplatePart::Literal {
                value: "review".into(),
            }]),
            output: None,
        }),
        access: WorkspaceAccess::Mutating,
        allow_failure: false,
        timeout_seconds: 60,
        max_output_bytes: 1024,
    };
    let graph = WorkflowGraph {
        name: WorkflowName::new("test").unwrap(),
        nodes: BTreeMap::from([(node.id.clone(), node)]),
    };
    let mut workflow = FrozenWorkflow {
        schema_version: FROZEN_WORKFLOW_SCHEMA_VERSION,
        planner: PlannerIdentity {
            name: "rho".into(),
            format_version: 1,
            starlark_version: "0.14.2".into(),
        },
        graph_digest: Digest(String::new()),
        sources: SourceManifest {
            entry_label: "//workflow.star".into(),
            modules: BTreeMap::from([(
                "//workflow.star".into(),
                SourceFile {
                    digest: Digest(
                        "sha256:a5c059fd4fd0193f7778541d9f8baecd730bbb76a1b3ed86ca5a5eeea33085b6"
                            .into(),
                    ),
                    bytes: 15,
                },
            )]),
        },
        inputs: BTreeMap::new(),
        graph,
        resolved_nodes: BTreeMap::from([(
            node_id("inspect"),
            ResolvedNode::Agent(Box::new(ResolvedAgent {
                agent_id: "reviewer".into(),
                fingerprint: "fingerprint".into(),
                runtime: AgentRuntime::Rho,
                source_origin: "builtin".into(),
                trust_required: false,
                prompt_policy: "review".into(),
                provider: None,
                model: None,
                reasoning: None,
                step_limit: 100,
                capabilities: Default::default(),
                permission_ceiling: "auto".into(),
                auth_profile: None,
                executable: None,
                executable_identity: None,
                arguments: Vec::new(),
            })),
        )]),
        scheduler: FrozenSchedulerSettings {
            max_parallel_nodes: 8,
            max_parallel_agents: 8,
            max_parallel_commands: 8,
        },
        runtime_limits: crate::workflow::test_support::runtime_limits(),
    };
    workflow.graph_digest = graph_digest(&workflow).unwrap();
    workflow
}

fn structured_workflow() -> FrozenWorkflow {
    let mut workflow = test_workflow();
    let NodeExecution::Agent(agent) = &mut workflow
        .graph
        .nodes
        .get_mut(&node_id("inspect"))
        .unwrap()
        .execution
    else {
        unreachable!();
    };
    agent.output = Some(OutputSchema::Bool);
    workflow.graph_digest = graph_digest(&workflow).unwrap();
    workflow
}

fn cancellation_workflow() -> FrozenWorkflow {
    let mut workflow = test_workflow();
    let mut follow_up = workflow.graph.nodes[&node_id("inspect")].clone();
    follow_up.id = node_id("report");
    follow_up.display_name = "report".into();
    follow_up.needs = vec![node_id("inspect")];
    workflow.graph.nodes.insert(follow_up.id.clone(), follow_up);
    workflow.resolved_nodes.insert(
        node_id("report"),
        workflow.resolved_nodes[&node_id("inspect")].clone(),
    );
    workflow.graph_digest = graph_digest(&workflow).unwrap();
    workflow
}

fn test_state(workflow: &FrozenWorkflow) -> WorkflowState {
    WorkflowState {
        revision: 0,
        lifecycle: RunLifecycle::Planned,
        outcome: None,
        cancellation_requested: false,
        nodes: workflow
            .graph
            .nodes
            .keys()
            .cloned()
            .map(|id| (id, NodeState::Pending))
            .collect(),
        command_exits: BTreeMap::new(),
        outputs: BTreeMap::new(),
        completions: BTreeMap::new(),
    }
}

fn create_run(home: &std::path::Path, workspace: &std::path::Path) -> StoredRun {
    create_run_with_workflow(home, workspace, test_workflow())
}

fn create_run_with_workflow(
    home: &std::path::Path,
    workspace: &std::path::Path,
    graph: FrozenWorkflow,
) -> StoredRun {
    let store = WorkflowStore::new(home).unwrap();
    let plan = store
        .create_plan(
            &graph,
            workspace
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            &BTreeMap::from([("//workflow.star".to_owned(), "WORKFLOW = None".to_owned())]),
        )
        .unwrap();
    store
        .create_run(
            &plan,
            PlanConsent {
                graph_digest: plan.manifest.graph_digest.clone(),
                confirmed: true,
            },
            RunStateRecord {
                schema_version: RUN_STATE_VERSION,
                last_event_sequence: 0,
                state: test_state(&plan.graph),
            },
        )
        .unwrap()
}

fn runner(
    home: &std::path::Path,
    workspace: &std::path::Path,
    executor: Arc<dyn WorkflowNodeExecutor>,
) -> WorkflowRunner {
    WorkflowRunner::new(
        home.to_owned(),
        workspace.to_owned(),
        RuntimeSecurity {
            project_trusted: true,
            permission_mode: crate::permission::PermissionMode::Auto,
        },
        Arc::clone(&executor),
        executor,
    )
}

fn append_fixture_event(
    store: &WorkflowStore,
    guard: &mut RunMutationGuard,
    run_directory: &std::path::Path,
    run: &mut StoredRun,
    event: WorkflowEvent,
) {
    let next =
        super::journal::apply_durable_event(&run.graph, run_directory, &run.state.state, &event)
            .unwrap();
    let sequence = run.state.last_event_sequence + 1;
    store
        .append_event(
            guard,
            &WorkflowEventRecord {
                schema_version: EVENT_VERSION,
                sequence,
                event,
            },
        )
        .unwrap();
    run.state.last_event_sequence = sequence;
    run.state.state = next;
    store.save_state(guard, &run.state).unwrap();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionCrashPoint {
    TerminalAttempt,
    StructuredOutput,
    NodeFinished,
    SnapshotSaved,
}

// Covers: each durable write boundary after execution could strand completed work or rerun it.
// Owner: workflow crash recovery.
#[tokio::test]
async fn terminal_completion_recovers_at_each_crash_point() {
    for point in [
        CompletionCrashPoint::TerminalAttempt,
        CompletionCrashPoint::StructuredOutput,
        CompletionCrashPoint::NodeFinished,
        CompletionCrashPoint::SnapshotSaved,
    ] {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let mut run =
            create_run_with_workflow(home.path(), workspace.path(), structured_workflow());
        let store = WorkflowStore::new(home.path()).unwrap();
        let run_directory = home
            .path()
            .join("workflows/runs")
            .join(run.manifest.run_id.to_string());
        let attempt = AttemptNumber::new(1).unwrap();
        let node = node_id("inspect");
        let mut guard = store.lock_run(run.manifest.run_id).unwrap();
        append_fixture_event(
            &store,
            &mut guard,
            &run_directory,
            &mut run,
            WorkflowEvent::RunLifecycle {
                lifecycle: RunLifecycle::Running,
            },
        );
        append_fixture_event(
            &store,
            &mut guard,
            &run_directory,
            &mut run,
            WorkflowEvent::NodeReady { node: node.clone() },
        );
        let attempt_directory = run_directory.join("nodes/inspect/attempts/1");
        std::fs::create_dir_all(&attempt_directory).unwrap();
        let output_artifact = super::artifacts::write_artifact(
            &run_directory,
            &attempt_directory.join("output.json"),
            b"true",
        )
        .unwrap();
        let output = ValidatedOutputRef {
            artifact: output_artifact.clone(),
            value: WorkflowValue::Bool(true),
        };
        let completion = NodeCompletion {
            attempt: Some(attempt),
            outcome: NodeTerminalState::Success,
            cancellation_resume: None,
            command_exit: None,
            structured_output: Some(output.clone()),
            artifacts: AttemptArtifacts {
                structured_output: Some(output_artifact),
                ..AttemptArtifacts::default()
            },
        };
        std::fs::write(
            attempt_directory.join("status.json"),
            serde_json::to_vec(&AttemptRecord {
                schema_version: ATTEMPT_VERSION,
                attempt,
                state: AttemptState::Completed {
                    completion: Box::new(completion.clone()),
                },
            })
            .unwrap(),
        )
        .unwrap();
        append_fixture_event(
            &store,
            &mut guard,
            &run_directory,
            &mut run,
            WorkflowEvent::AttemptStarted {
                node: node.clone(),
                attempt,
                owner: ExternalOwner::Process { pid: 42 },
            },
        );
        let structured_event = WorkflowEvent::StructuredOutput {
            node: node.clone(),
            attempt,
            output,
        };
        let finished_event = WorkflowEvent::NodeFinished {
            node,
            attempt: Some(attempt),
            outcome: completion.outcome,
        };
        match point {
            CompletionCrashPoint::TerminalAttempt => {}
            CompletionCrashPoint::StructuredOutput => {
                let sequence = run.state.last_event_sequence + 1;
                store
                    .append_event(
                        &mut guard,
                        &WorkflowEventRecord {
                            schema_version: EVENT_VERSION,
                            sequence,
                            event: structured_event,
                        },
                    )
                    .unwrap();
            }
            CompletionCrashPoint::NodeFinished => {
                for (offset, event) in [structured_event, finished_event].into_iter().enumerate() {
                    let sequence = run.state.last_event_sequence + offset as u64 + 1;
                    store
                        .append_event(
                            &mut guard,
                            &WorkflowEventRecord {
                                schema_version: EVENT_VERSION,
                                sequence,
                                event,
                            },
                        )
                        .unwrap();
                }
            }
            CompletionCrashPoint::SnapshotSaved => {
                append_fixture_event(
                    &store,
                    &mut guard,
                    &run_directory,
                    &mut run,
                    structured_event,
                );
                append_fixture_event(&store, &mut guard, &run_directory, &mut run, finished_event);
            }
        }
        drop(guard);

        let executor = Arc::new(CountingExecutor(AtomicUsize::new(0)));
        let executor_trait: Arc<dyn WorkflowNodeExecutor> = executor.clone();
        let completed = runner(home.path(), workspace.path(), executor_trait)
            .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
            .await
            .unwrap();

        assert_eq!(executor.0.load(Ordering::SeqCst), 0, "point: {point:?}");
        assert_eq!(
            completed.state.state.nodes[&node_id("inspect")].terminal(),
            Some(NodeTerminalState::Success),
            "point: {point:?}"
        );
        assert_eq!(
            completed.state.state.outputs[&node_id("inspect")],
            WorkflowValue::Bool(true),
            "point: {point:?}"
        );
        assert_eq!(
            completed.state.state.outcome,
            Some(WorkflowOutcome::Success),
            "point: {point:?}"
        );
        let artifact = completed.state.state.completions[&node_id("inspect")]
            .artifacts
            .structured_output
            .as_ref()
            .unwrap();
        assert_eq!(artifact.retained_bytes, 4, "point: {point:?}");
        assert_eq!(
            artifact.observed,
            ArtifactObservation::Complete { observed_bytes: 4 },
            "point: {point:?}"
        );

        let mut replayed = StoredRun {
            manifest: completed.manifest.clone(),
            graph: completed.graph.clone(),
            state: RunStateRecord {
                schema_version: RUN_STATE_VERSION,
                last_event_sequence: 0,
                state: test_state(&completed.graph),
            },
        };
        super::journal::replay_journal(&store, &run_directory, &mut replayed).unwrap();
        assert_eq!(replayed.state, completed.state, "point: {point:?}");
        if point == CompletionCrashPoint::SnapshotSaved {
            std::fs::write(run_directory.join(&artifact.relative_path), b"false").unwrap();
            assert!(matches!(
                store.load_run(completed.manifest.run_id),
                Err(WorkflowError::Corrupt { .. })
            ));
        }
    }
}

// Covers: a node result must become a durable terminal state and attempt record.
// Owner: durable workflow runner.
#[tokio::test]
async fn runner_persists_successful_attempt() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run(home.path(), workspace.path());
    let runner = runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor));

    let completed = runner
        .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
        .await
        .unwrap();

    assert_eq!(completed.state.state.lifecycle, RunLifecycle::Completed);
    assert_eq!(
        completed.state.state.nodes[&node_id("inspect")].terminal(),
        Some(NodeTerminalState::Success)
    );
    let attempt: AttemptRecord = serde_json::from_slice(
        &std::fs::read(
            home.path()
                .join("workflows/runs")
                .join(run.manifest.run_id.to_string())
                .join("nodes/inspect/attempts/1/status.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        attempt.state,
        AttemptState::Completed {
            completion: Box::new(NodeCompletion {
                attempt: Some(AttemptNumber::new(1).unwrap()),
                ..NodeCompletion::terminal(NodeTerminalState::Success)
            })
        }
    );
}

// Covers: artifact writes must not follow a substituted parent or final symlink.
// Owner: durable workflow artifact storage.
#[cfg(unix)]
#[test]
fn artifact_writes_do_not_follow_symlinks() {
    use std::os::unix::fs::symlink;

    let run = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let attempt = run.path().join("nodes/inspect/attempts/1");
    crate::workflow::ensure_directory_beneath(
        run.path(),
        std::path::Path::new("nodes/inspect/attempts/1"),
    )
    .unwrap();
    let agent = attempt.join("agent");
    symlink(outside.path(), &agent).unwrap();
    assert!(
        super::artifacts::write_artifact(run.path(), &agent.join("answer.txt"), b"blocked",)
            .is_err()
    );

    std::fs::remove_file(&agent).unwrap();
    std::fs::create_dir(&agent).unwrap();
    let outside_file = outside.path().join("outside.txt");
    std::fs::write(&outside_file, b"outside").unwrap();
    let answer = agent.join("answer.txt");
    symlink(&outside_file, &answer).unwrap();
    super::artifacts::write_artifact(run.path(), &answer, b"inside").unwrap();

    assert_eq!(std::fs::read(&outside_file).unwrap(), b"outside");
    assert_eq!(std::fs::read(&answer).unwrap(), b"inside");
    assert!(!std::fs::symlink_metadata(&answer)
        .unwrap()
        .file_type()
        .is_symlink());
}

// Covers: a separate CLI process can cancel an active node through the durable request file.
// Owner: durable workflow runner cancellation.
#[tokio::test]
async fn cross_process_request_cancels_active_node() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run_with_workflow(home.path(), workspace.path(), cancellation_workflow());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let executor = Arc::new(CancellationExecutor {
        started: std::sync::Mutex::new(Some(started_tx)),
    });
    let active_runner = Arc::new(runner(home.path(), workspace.path(), executor));
    let worker_runner = Arc::clone(&active_runner);
    let run_id = run.manifest.run_id;
    let worker = tokio::spawn(async move {
        worker_runner
            .drive(run_id, RecoveryDecision::NormalResume, None)
            .await
    });

    started_rx.await.unwrap();
    let receipt = WorkflowRunner::request_cross_process_cancel(home.path(), run_id).unwrap();
    // Receipt: this is a generous tripwire above the measured 87 ms CLI cancellation.
    let completed = tokio::time::timeout(std::time::Duration::from_secs(2), worker)
        .await
        .expect("cross-process cancellation exceeded the 2 second test budget")
        .unwrap()
        .unwrap();

    assert_eq!(completed.state.state.lifecycle, RunLifecycle::Completed);
    assert!(completed.state.state.cancellation_requested);
    assert_eq!(
        completed.state.state.nodes[&node_id("inspect")].terminal(),
        Some(NodeTerminalState::Cancellation)
    );
    assert_eq!(
        completed.state.state.nodes[&node_id("report")].terminal(),
        Some(NodeTerminalState::Cancellation)
    );
    assert_eq!(
        completed.state.state.outcome,
        Some(WorkflowOutcome::Cancellation)
    );
    assert!(completed
        .state
        .state
        .nodes
        .values()
        .all(|state| state.terminal().is_some()));
    assert!(WorkflowRunner::cross_process_cancel_acknowledged(home.path(), run_id).unwrap());
    assert!(
        WorkflowRunner::cancellation_request_acknowledged(home.path(), run_id, &receipt).unwrap()
    );

    let resumed = runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor))
        .drive(run_id, RecoveryDecision::NormalResume, None)
        .await
        .unwrap();
    assert_eq!(resumed.state.state.outcome, Some(WorkflowOutcome::Success));
    assert!(resumed
        .state
        .state
        .nodes
        .values()
        .all(|state| { state.terminal() == Some(NodeTerminalState::Success) }));

    let store = WorkflowStore::new(home.path()).unwrap();
    let run_directory = home.path().join("workflows/runs").join(run_id.to_string());
    let mut replayed = StoredRun {
        manifest: resumed.manifest.clone(),
        graph: resumed.graph.clone(),
        state: RunStateRecord {
            schema_version: RUN_STATE_VERSION,
            last_event_sequence: 0,
            state: test_state(&resumed.graph),
        },
    };
    super::journal::replay_journal(&store, &run_directory, &mut replayed).unwrap();
    assert_eq!(replayed.state, resumed.state);

    let unobserved = WorkflowRunner::request_cross_process_cancel(home.path(), run_id).unwrap();
    assert!(
        !WorkflowRunner::cancellation_request_acknowledged(home.path(), run_id, &unobserved)
            .unwrap()
    );
}

// Covers: a flushed event after the last snapshot must replay before scheduling resumes.
// Owner: workflow journal replay.
#[tokio::test]
async fn runner_replays_journal_tail() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run(home.path(), workspace.path());
    let store = WorkflowStore::new(home.path()).unwrap();
    let mut guard = store.lock_run(run.manifest.run_id).unwrap();
    store
        .append_event(
            &mut guard,
            &WorkflowEventRecord {
                schema_version: EVENT_VERSION,
                sequence: 1,
                event: WorkflowEvent::NodeReady {
                    node: node_id("inspect"),
                },
            },
        )
        .unwrap();
    drop(guard);
    std::fs::OpenOptions::new()
        .append(true)
        .open(
            home.path()
                .join("workflows/runs")
                .join(run.manifest.run_id.to_string())
                .join("events.jsonl"),
        )
        .unwrap()
        .write_all(b"{\"schema_version\":")
        .unwrap();

    let runner = runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor));
    let completed = runner
        .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
        .await
        .unwrap();

    assert_eq!(completed.state.state.lifecycle, RunLifecycle::Completed);
    assert_eq!(
        completed.state.state.nodes[&node_id("inspect")].terminal(),
        Some(NodeTerminalState::Success)
    );
    assert!(completed.state.last_event_sequence > 1);
    let records = store.read_events(run.manifest.run_id).unwrap();
    assert_eq!(records.last().unwrap().sequence, records.len() as u64);
}

// Covers: an attacker-controlled checkout lock symlink must not be opened or replaced.
// Owner: checkout gate filesystem lock boundary.
#[cfg(unix)]
#[test]
fn checkout_gate_rejects_symlink_lock() {
    use sha2::{Digest as _, Sha256};
    use std::os::unix::fs::symlink;

    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let locks = home.path().join("workflows/checkout-locks");
    std::fs::create_dir_all(&locks).unwrap();
    let canonical = workspace.path().canonicalize().unwrap();
    let key = format!(
        "{:x}",
        Sha256::digest(canonical.to_string_lossy().as_bytes())
    );
    let target = home.path().join("attacker-file");
    std::fs::write(&target, "do not open").unwrap();
    symlink(&target, locks.join(format!("{key}.lock"))).unwrap();

    assert!(CheckoutGate::new(home.path(), workspace.path()).is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "do not open");
}

// Covers: a prior running attempt must stop resume until the caller confirms no process exists.
// Owner: workflow crash recovery.
#[tokio::test]
async fn uncertain_attempt_requires_explicit_recovery() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let mut run = create_run(home.path(), workspace.path());
    let node = node_id("inspect");
    let attempt = AttemptNumber::new(1).unwrap();
    run.state.state.lifecycle = RunLifecycle::Running;
    run.state
        .state
        .nodes
        .insert(node, NodeState::Running { attempt });
    let store = WorkflowStore::new(home.path()).unwrap();
    let guard = store.lock_run(run.manifest.run_id).unwrap();
    store.save_state(&guard, &run.state).unwrap();
    drop(guard);
    let attempt_directory = home
        .path()
        .join("workflows/runs")
        .join(run.manifest.run_id.to_string())
        .join("nodes/inspect/attempts/1");
    std::fs::create_dir_all(&attempt_directory).unwrap();
    std::fs::write(
        attempt_directory.join("status.json"),
        serde_json::to_vec(&AttemptRecord {
            schema_version: ATTEMPT_VERSION,
            attempt,
            state: AttemptState::Started {
                owner: ExternalOwner::Process { pid: 4242 },
            },
        })
        .unwrap(),
    )
    .unwrap();

    let runner = runner(home.path(), workspace.path(), Arc::new(SuccessfulExecutor));
    let error = runner
        .drive(run.manifest.run_id, RecoveryDecision::NormalResume, None)
        .await
        .unwrap_err();
    assert!(matches!(error, RuntimeError::NeedsRecovery { .. }));
    assert_eq!(
        store
            .load_run(run.manifest.run_id)
            .unwrap()
            .state
            .state
            .lifecycle,
        RunLifecycle::NeedsRecovery
    );
    let interrupted: AttemptRecord =
        serde_json::from_slice(&std::fs::read(attempt_directory.join("status.json")).unwrap())
            .unwrap();
    assert_eq!(
        interrupted.state,
        AttemptState::InterruptedUncertain {
            owner: ExternalOwner::Process { pid: 4242 }
        }
    );
}
