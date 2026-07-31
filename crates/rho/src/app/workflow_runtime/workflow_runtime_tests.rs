use std::{collections::BTreeMap, sync::Arc};

use crate::workflow::*;

use super::*;

#[derive(Default)]
struct SuccessfulExecutor;

impl WorkflowNodeExecutor for SuccessfulExecutor {
    fn execute<'a>(&'a self, _request: NodeExecutionRequest) -> WorkflowExecutionFuture<'a> {
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
                arguments: Vec::new(),
            })),
        )]),
        scheduler: FrozenSchedulerSettings {
            max_parallel_nodes: 8,
            max_parallel_agents: 8,
            max_parallel_commands: 8,
        },
    };
    workflow.graph_digest = graph_digest(&workflow).unwrap();
    workflow
}

fn test_state(workflow: &FrozenWorkflow) -> WorkflowState {
    WorkflowState {
        revision: 0,
        lifecycle: RunLifecycle::Planned,
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
    }
}

fn create_run(home: &std::path::Path, workspace: &std::path::Path) -> StoredRun {
    let store = WorkflowStore::new(home).unwrap();
    let graph = test_workflow();
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
            outcome: NodeTerminalState::Success
        }
    );
}

// Covers: a separate CLI process can cancel an active node through the durable request file.
// Owner: durable workflow runner cancellation.
#[tokio::test]
async fn cross_process_request_cancels_active_node() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = create_run(home.path(), workspace.path());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let executor = Arc::new(CancellationExecutor {
        started: std::sync::Mutex::new(Some(started_tx)),
    });
    let runner = Arc::new(runner(home.path(), workspace.path(), executor));
    let worker_runner = Arc::clone(&runner);
    let run_id = run.manifest.run_id;
    let worker = tokio::spawn(async move {
        worker_runner
            .drive(run_id, RecoveryDecision::NormalResume, None)
            .await
    });

    started_rx.await.unwrap();
    WorkflowRunner::request_cross_process_cancel(home.path(), run_id).unwrap();
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
