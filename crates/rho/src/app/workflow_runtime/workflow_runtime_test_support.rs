use super::*;
use crate::workflow::*;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

#[derive(Default)]
pub(super) struct SuccessfulExecutor;
impl<I: Send + 'static> WorkflowNodeExecutor<I> for SuccessfulExecutor {
    fn execute<'a>(&'a self, _request: NodeExecutionRequest<I>) -> WorkflowExecutionFuture<'a> {
        Box::pin(async { Ok(NodeExecutionResult::terminal(NodeTerminalState::Success)) })
    }
}
pub(super) struct CountingExecutor(pub(super) AtomicUsize);
impl<I: Send + 'static> WorkflowNodeExecutor<I> for CountingExecutor {
    fn execute<'a>(&'a self, _request: NodeExecutionRequest<I>) -> WorkflowExecutionFuture<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(NodeExecutionResult::terminal(NodeTerminalState::Success)) })
    }
}
pub(super) struct CancellationExecutor {
    pub(super) started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}
impl<I: Send + 'static> WorkflowNodeExecutor<I> for CancellationExecutor {
    fn execute<'a>(&'a self, request: NodeExecutionRequest<I>) -> WorkflowExecutionFuture<'a> {
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
pub(super) struct NeverCompletingAgentExecutor {
    pub(super) cancelled: AtomicBool,
}
impl super::agent::AgentCleanupHandle for NeverCompletingAgentExecutor {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
    fn wait(&mut self) -> super::agent::AgentCleanupFuture<'_> {
        Box::pin(std::future::pending())
    }
}
pub(super) struct UncertainCleanupExecutor {
    pub(super) started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    pub(super) cleanup_started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    // oneshot cannot lose a wake sent before the wait is registered.
    pub(super) release_cleanup: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
impl<I: Send + 'static> WorkflowNodeExecutor<I> for UncertainCleanupExecutor {
    fn execute<'a>(&'a self, request: NodeExecutionRequest<I>) -> WorkflowExecutionFuture<'a> {
        Box::pin(async move {
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            request.cancellation.cancelled().await;
            if let Some(started) = self.cleanup_started.lock().unwrap().take() {
                let _ = started.send(());
            }
            let release = self
                .release_cleanup
                .lock()
                .unwrap()
                .take()
                .expect("release installed before execute");
            release
                .await
                .expect("cleanup release closed before release");
            Err(RuntimeError::CleanupUncertain {
                cause: CleanupCause::Cancellation,
            })
        })
    }
}
pub(super) struct SignallingSuccessfulExecutor {
    pub(super) started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}
impl<I: Send + 'static> WorkflowNodeExecutor<I> for SignallingSuccessfulExecutor {
    fn execute<'a>(&'a self, _request: NodeExecutionRequest<I>) -> WorkflowExecutionFuture<'a> {
        if let Some(started) = self.started.lock().unwrap().take() {
            let _ = started.send(());
        }
        Box::pin(async { Ok(NodeExecutionResult::terminal(NodeTerminalState::Success)) })
    }
}
pub(super) struct TimeoutCleanupExecutor;
impl<I: Send + 'static> WorkflowNodeExecutor<I> for TimeoutCleanupExecutor {
    fn execute<'a>(&'a self, _request: NodeExecutionRequest<I>) -> WorkflowExecutionFuture<'a> {
        Box::pin(async {
            Err(RuntimeError::CleanupUncertain {
                cause: CleanupCause::Timeout,
            })
        })
    }
}
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
pub(super) struct AllowCommandHosts;
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
impl CommandHostFactory for AllowCommandHosts {
    fn create(
        &self,
        tool: crate::tools::process::WorkflowCommandTool,
        labels: rho_sdk::hooks::HookHostLabels,
    ) -> Result<rho_sdk::ToolHost, RuntimeError> {
        rho_sdk::ToolHost::builder()
            .tool(tool)
            .workspace_policy(crate::app::policy::AppPolicy::Allow)
            .hook_host_labels(labels)
            .build()
            .map_err(|error| RuntimeError::Executor(error.to_string()))
    }
}
pub(super) fn node_id(value: &str) -> NodeId {
    NodeId::new(value).unwrap()
}
pub(super) fn task_id(value: &str) -> TaskInstanceId {
    TaskInstanceId::root(node_id(value))
}
pub(super) fn terminal(run: &StoredRun, node: &str) -> Option<NodeTerminalState> {
    run.state.state.task(&task_id(node)).unwrap().terminal()
}
pub(super) fn completion<'a>(run: &'a StoredRun, node: &str) -> &'a NodeCompletion {
    run.state.state.completion(&task_id(node)).unwrap()
}
pub(super) fn root_output<'a>(run: &'a StoredRun, node: &str) -> &'a WorkflowValue {
    &run.state.state.root_scope().outputs[&node_id(node)]
}
pub(super) fn test_workflow() -> FrozenWorkflow {
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
        // Product failure bound, not a synchronization signal.
        timeout_seconds: 5,
        max_output_bytes: 1024,
    };
    let program = WorkflowProgram {
        name: WorkflowName::new("test").unwrap(),
        root: ScopeDefinition {
            parameters: BTreeMap::new(),
            nodes: BTreeMap::from([(node.id.clone(), node)]),
            exports: BTreeMap::new(),
        },
    };
    let mut workflow = FrozenWorkflow {
        schema_version: FROZEN_WORKFLOW_SCHEMA_VERSION,
        planner: PlannerIdentity {
            name: "rho".into(),
            format_version: 1,
            starlark_version: "0.14.2".into(),
        },
        program_digest: Digest(String::new()),
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
        program,
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
    workflow.program_digest = program_digest(&workflow).unwrap();
    workflow
}
pub(super) fn structured_workflow() -> FrozenWorkflow {
    let mut workflow = test_workflow();
    let NodeExecution::Agent(agent) = &mut workflow
        .program
        .root
        .nodes
        .get_mut(&node_id("inspect"))
        .unwrap()
        .execution
    else {
        unreachable!()
    };
    agent.output = Some(OutputSchema::Bool);
    workflow.program_digest = program_digest(&workflow).unwrap();
    workflow
}
pub(super) fn cancellation_workflow() -> FrozenWorkflow {
    let mut workflow = test_workflow();
    let mut follow_up = workflow.program.root.nodes[&node_id("inspect")].clone();
    follow_up.id = node_id("report");
    follow_up.display_name = "report".into();
    follow_up.needs = vec![node_id("inspect")];
    workflow
        .program
        .root
        .nodes
        .insert(follow_up.id.clone(), follow_up);
    workflow.resolved_nodes.insert(
        node_id("report"),
        workflow.resolved_nodes[&node_id("inspect")].clone(),
    );
    workflow.program_digest = program_digest(&workflow).unwrap();
    workflow
}
// Frozen handle-based command launch is Linux/Android-only.
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
pub(super) fn command_workflow(
    workspace: &std::path::Path,
    ready: &std::path::Path,
    marker: &std::path::Path,
) -> FrozenWorkflow {
    let mut workflow = test_workflow();
    let executable = std::path::Path::new("/bin/sh").canonicalize().unwrap();
    let cwd = workspace.canonicalize().unwrap();
    let quote = |path: &std::path::Path| shell_words::quote(&path.to_string_lossy()).into_owned();
    // Park on sleep until cancellation; regular-file readiness avoids blocking FIFO opens.
    let script = format!("if test -f {marker}; then printf resumed; printf done >&2; exit 0; fi; : > {marker}; printf first; printf err >&2; : > {ready}; exec sleep 1000", marker = quote(marker), ready = quote(ready));
    let node = workflow
        .program
        .root
        .nodes
        .get_mut(&node_id("inspect"))
        .unwrap();
    node.execution = NodeExecution::Command(CommandNode::Shell {
        executable: executable.to_string_lossy().into_owned(),
        arguments: vec!["-c".into()],
        command: script,
        cwd: ".".into(),
        output: None,
    });
    node.max_output_bytes = 4;
    workflow.resolved_nodes.insert(
        node_id("inspect"),
        ResolvedNode::Command(Box::new(ResolvedCommand {
            executable: executable.to_string_lossy().into_owned(),
            executable_identity: freeze_executable_identity(&executable).unwrap(),
            exact_path: true,
            cwd: cwd.to_string_lossy().into_owned(),
            cwd_identity: freeze_directory_identity(&cwd).unwrap(),
            environment_policy: "empty".into(),
        })),
    );
    workflow.program_digest = program_digest(&workflow).unwrap();
    workflow
}
pub(super) fn create_run(home: &std::path::Path, workspace: &std::path::Path) -> StoredRun {
    create_run_with_workflow(home, workspace, test_workflow())
}
pub(super) fn create_run_with_workflow(
    home: &std::path::Path,
    workspace: &std::path::Path,
    graph: FrozenWorkflow,
) -> StoredRun {
    let store = WorkflowStore::new(home).unwrap();
    let plan = store
        .create_plan(
            &graph,
            // Match production canonical display so Windows path forms compare equal.
            crate::paths::display(&workspace.canonicalize().unwrap()),
            &BTreeMap::from([("//workflow.star".to_owned(), "WORKFLOW = None".to_owned())]),
        )
        .unwrap();
    store
        .create_run(
            &plan,
            PlanConsent {
                program_digest: plan.manifest.program_digest.clone(),
                confirmed: true,
            },
            RunStateRecord {
                schema_version: RUN_STATE_VERSION,
                last_event_sequence: 0,
                state: WorkflowState::new(&plan.graph),
            },
        )
        .unwrap()
}
pub(super) fn runner<
    E: WorkflowNodeExecutor<AgentInvocation> + WorkflowNodeExecutor<CommandInvocation> + 'static,
>(
    home: &std::path::Path,
    workspace: &std::path::Path,
    executor: Arc<E>,
) -> WorkflowRunner {
    WorkflowRunner::new(
        home.to_owned(),
        workspace.to_owned(),
        RuntimeSecurity {
            project_trusted: true,
            permission_mode: crate::permission::PermissionMode::Auto,
        },
        executor.clone(),
        executor,
    )
}
pub(super) fn append_fixture_event(
    store: &WorkflowStore,
    guard: &mut RunMutationGuard,
    run_directory: &std::path::Path,
    run: &mut StoredRun,
    event: WorkflowEvent,
) {
    let next = apply_durable_event(&run.graph, &run.state.state, &event, run_directory).unwrap();
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
pub(super) fn assert_replay(home: &std::path::Path, run: &StoredRun) {
    let records = WorkflowStore::new(home)
        .unwrap()
        .read_events(run.manifest.run_id)
        .unwrap();
    assert_eq!(
        derive_snapshot(
            &run.graph,
            &records,
            run.state.last_event_sequence,
            &WorkflowLayout::new(home).run(run.manifest.run_id)
        )
        .unwrap(),
        run.state.state
    );
    assert_eq!(
        records.last().unwrap().sequence,
        run.state.last_event_sequence
    );
}

// Receipt: Ubuntu lib tests ~14s; Windows suite hit 5s (~204s suite total).
// A generous failure tripwire around event-driven completion, never synchronization.
const WORKER_COMPLETION_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);
pub(super) async fn within_budget<T>(
    label: &str,
    future: impl std::future::Future<Output = T>,
) -> T {
    tokio::time::timeout(WORKER_COMPLETION_BUDGET, future)
        .await
        .unwrap_or_else(|_| {
            panic!("{label} exceeded WORKER_COMPLETION_BUDGET ({WORKER_COMPLETION_BUDGET:?})")
        })
}
/// Abort on timeout so detached tasks cannot retain run/checkout locks after failure.
pub(super) async fn join_within_budget<T: 'static>(
    label: &str,
    handle: tokio::task::JoinHandle<T>,
) -> T {
    tokio::pin!(handle);
    tokio::select! {
        result = &mut handle => result.unwrap_or_else(|error| panic!("{label} task failed: {error}")),
        () = tokio::time::sleep(WORKER_COMPLETION_BUDGET) => {
            handle.abort(); let _ = handle.await;
            panic!("{label} exceeded WORKER_COMPLETION_BUDGET ({WORKER_COMPLETION_BUDGET:?})");
        }
    }
}
