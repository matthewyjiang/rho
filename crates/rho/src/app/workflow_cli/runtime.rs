use std::{
    future::Future,
    io::{self, IsTerminal, Write},
    pin::Pin,
    sync::Arc,
};

use rho_sdk::{
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, ApprovalSession,
    CapabilityOperation, ProcessEnvironment, ToolHost, Workspace,
};
use sha2::{Digest as _, Sha256};

use crate::{
    app::{
        agent_executor::AgentExecutor,
        bootstrap::absolute_config_path,
        config_repository::ConfigRepository,
        policy::AppPolicy,
        subagent_host_input::SubagentHostInputBridge,
        workflow_runtime::{
            CommandHostFactory, RecoveryDecision, RuntimeError, RuntimeEvent, RuntimeSecurity,
            WorkflowAgentExecutor, WorkflowCommandExecutor, WorkflowNodeExecutor, WorkflowRunner,
        },
    },
    cli::WorkflowRunFormat,
    tui::workflow::{
        CancellationState, ExecutionMetadata, PlanApprovalState, SourceDigestSummary,
        TerminalReason, WorkflowAction, WorkflowEvent as TuiEvent, WorkflowEventAdapter,
        WorkflowNodeSnapshot, WorkflowProgress, WorkflowSession, WorkflowSnapshot,
    },
    workflow::{
        derive_workflow_outcome, CommandNode, Digest, NodeExecution, NodeState, NodeTerminalState,
        ResolvedNode, RunId, RunLifecycle, StoredRun, Template, TemplatePart, WorkflowStore,
    },
};

use super::{write_json_document, WORKFLOW_WIRE_VERSION};

#[derive(Clone, Copy)]
enum RuntimePresentation {
    Text,
    Jsonl,
}

struct TerminalWorkflowApprovals {
    interactive: bool,
}

impl ApprovalHandler for TerminalWorkflowApprovals {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a> {
        if !self.interactive {
            return Box::pin(std::future::ready(ApprovalDecision::Deny {
                reason: "workflow capability approval requires an interactive terminal".into(),
            }));
        }
        Box::pin(async move {
            tokio::task::spawn_blocking(move || prompt_for_capability(request))
                .await
                .unwrap_or_else(|error| ApprovalDecision::Deny {
                    reason: format!("workflow approval prompt failed: {error}"),
                })
        })
    }
}

fn prompt_for_capability(request: ApprovalRequest) -> ApprovalDecision {
    eprintln!(
        "workflow requests {} capability from {:?}",
        request.capability().kind().label(),
        request.capability().source()
    );
    match request.capability().operation() {
        CapabilityOperation::ReadPath { path, scope } => {
            eprintln!("read path: {} ({scope:?})", path.display());
        }
        CapabilityOperation::WritePath { path, scope } => {
            eprintln!("write path: {} ({scope:?})", path.display());
        }
        CapabilityOperation::ExecuteProcess(process) => {
            eprintln!(
                "working directory: {}",
                process.working_directory().display()
            );
            eprintln!(
                "executable: {}",
                process.invocation().executable_path().display()
            );
            eprintln!("arguments: {:?}", process.invocation().arguments());
            eprintln!("environment: {:?}", process.environment());
            eprintln!("output limits: {:?}", process.output_limits());
        }
        operation => eprintln!("capability details: {operation:?}"),
    }
    if !request.reason().is_empty() {
        eprintln!("reason: {}", request.reason());
    }
    eprint!("allow [o]nce, allow for [s]ession (exact request), or [d]eny? ");
    if io::stderr().flush().is_err() {
        return ApprovalDecision::Deny {
            reason: "workflow approval prompt could not write to the terminal".into(),
        };
    }
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return ApprovalDecision::Deny {
            reason: "workflow approval prompt could not read from the terminal".into(),
        };
    }
    match answer.trim().to_ascii_lowercase().as_str() {
        "o" | "once" => ApprovalDecision::AllowOnce,
        "s" | "session" => ApprovalDecision::AllowForSession,
        _ => ApprovalDecision::Deny {
            reason: "workflow capability denied by user".into(),
        },
    }
}

struct WorkflowCommandHosts {
    workspace: Workspace,
    policy: AppPolicy,
    approvals: ApprovalSession,
    hooks: Option<crate::hooks::HookPipeline>,
}

impl CommandHostFactory for WorkflowCommandHosts {
    fn create(
        &self,
        tool: crate::tools::process::WorkflowCommandTool,
        labels: rho_sdk::hooks::HookHostLabels,
    ) -> Result<ToolHost, RuntimeError> {
        let mut builder = ToolHost::builder()
            .tool(tool)
            .workspace(self.workspace.clone())
            .workspace_policy(self.policy)
            .approval_session(self.approvals.clone())
            .hook_host_labels(labels);
        if let Some(hooks) = &self.hooks {
            builder = hooks.attach_tool_host(builder);
        }
        builder
            .build()
            .map_err(|error| RuntimeError::Executor(error.to_string()))
    }
}

pub(crate) async fn execute_run(
    run: StoredRun,
    recovery: RecoveryDecision,
    output: Option<WorkflowRunFormat>,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let interactive_input = io::stdin().is_terminal();
    let interactive_terminal = interactive_input && io::stdout().is_terminal();
    let interactive_display = io::stderr().is_terminal();
    let presentation = match output {
        Some(WorkflowRunFormat::Jsonl) => RuntimePresentation::Jsonl,
        Some(WorkflowRunFormat::Text) | None => RuntimePresentation::Text,
    };
    let rho_home = crate::paths::rho_dir()?;
    let approvals = ApprovalSession::new(TerminalWorkflowApprovals {
        interactive: interactive_input && interactive_display,
    });
    let runtime = WorkflowRuntime::build(&run, config_path, approvals)?;
    let use_tui = output.is_none()
        && interactive_terminal
        && interactive_display
        && runtime.permission_mode != crate::permission::PermissionMode::Supervised;
    let runner = Arc::clone(&runtime.runner);
    let execution = if use_tui {
        let adapter = RunnerTuiAdapter::start(Arc::clone(&runner), rho_home, run.clone(), recovery);
        crate::tui::workflow::run(Box::new(adapter))
            .await
            .map(|_| false)
    } else {
        drive_with_stream(Arc::clone(&runner), &run, recovery, presentation).await
    };
    drop(runner);
    runtime.shutdown().await;
    let interrupted = execution?;
    if interrupted {
        anyhow::bail!(
            "workflow was cancelled by an interrupt; resume it with `rho workflow resume {}`",
            run.manifest.run_id
        );
    }
    Ok(())
}

/// Starts a workflow run on a detached task and returns immediately.
///
/// The caller keeps chatting or returns a tool result while the run continues.
/// Inspect progress with status; stop it with cancel. The owner process must
/// stay alive for the run to make progress.
pub(crate) async fn spawn_background_run(
    run: StoredRun,
    recovery: RecoveryDecision,
    config_path: Option<std::path::PathBuf>,
    approvals: ApprovalSession,
) -> anyhow::Result<StoredRun> {
    let run_id = run.manifest.run_id;
    let runtime = WorkflowRuntime::build(&run, config_path, approvals)?;
    let runner = Arc::clone(&runtime.runner);
    tokio::spawn(async move {
        let result = runner.drive(run_id, recovery, None).await;
        drop(runner);
        runtime.shutdown().await;
        match result {
            Ok(_) => tracing::info!(%run_id, "background workflow completed"),
            Err(error) => {
                tracing::warn!(%run_id, error = %error, "background workflow failed")
            }
        }
    });
    // Return the pre-spawn snapshot immediately. Progress is eventually consistent
    // via status / watch; do not sleep or yield hoping the driver advanced.
    Ok(run)
}

struct WorkflowRuntime {
    runner: Arc<WorkflowRunner>,
    command_executor: Arc<dyn WorkflowNodeExecutor>,
    hosts: Arc<WorkflowCommandHosts>,
    permission_mode: crate::permission::PermissionMode,
}

impl WorkflowRuntime {
    fn build(
        run: &StoredRun,
        config_path: Option<std::path::PathBuf>,
        approvals: ApprovalSession,
    ) -> anyhow::Result<Self> {
        let cwd = std::env::current_dir()?.canonicalize()?;
        let repository = ConfigRepository::new(config_path);
        let config_path = absolute_config_path(&repository)?;
        let mut config = repository.load()?;
        let permission_mode = effective_permission_mode(run, config.permission_mode)?;
        config.permission_mode = permission_mode;
        let needs_provider_credentials = run.graph.resolved_nodes.values().any(|node| {
            matches!(
                node,
                ResolvedNode::Agent(agent)
                    if agent.runtime == crate::workflow::AgentRuntime::Rho
            )
        });
        if needs_provider_credentials {
            crate::credential_store::initialize_from_config(&mut config, &config_path)?;
        }
        let workspace = Workspace::new(&cwd)?.with_unrestricted_file_access();
        let hooks = crate::hooks::start_for_cwd(&cwd);
        let hook_engine = hooks.as_ref().map(|pipeline| Arc::clone(pipeline.engine()));
        let hosts = Arc::new(WorkflowCommandHosts {
            workspace,
            policy: AppPolicy::for_mode(permission_mode),
            approvals: approvals.clone(),
            hooks,
        });
        let process_environment = ProcessEnvironment::inherit_except(
            rho_providers::credential_env_vars().iter().copied(),
        );
        let app_agent_executor = Arc::new(
            AgentExecutor::new(
                config.clone(),
                config_path,
                cwd.clone(),
                SubagentHostInputBridge::new(),
            )
            .with_approval_session(approvals),
        );
        let agent_executor: Arc<dyn WorkflowNodeExecutor> =
            Arc::new(WorkflowAgentExecutor::new(app_agent_executor));
        let command_executor: Arc<dyn WorkflowNodeExecutor> =
            Arc::new(WorkflowCommandExecutor::new(
                process_environment,
                Arc::clone(&hosts) as Arc<dyn CommandHostFactory>,
            ));
        let security = RuntimeSecurity {
            project_trusted: std::env::var_os("RHO_TRUST_PROJECT_AGENTS").as_deref()
                == Some(std::ffi::OsStr::new("1")),
            permission_mode,
        };
        let mut runner = WorkflowRunner::new(
            crate::paths::rho_dir()?,
            cwd,
            security,
            agent_executor,
            Arc::clone(&command_executor),
        );
        if let Some(engine) = hook_engine {
            runner = runner.with_hooks(engine);
        }
        Ok(Self {
            runner: Arc::new(runner),
            command_executor,
            hosts,
            permission_mode,
        })
    }

    async fn shutdown(self) {
        drop(self.runner);
        drop(self.command_executor);
        match Arc::try_unwrap(self.hosts) {
            Ok(hosts) => {
                if let Some(hooks) = hosts.hooks {
                    hooks.shutdown(crate::hooks::DRAIN_GRACE).await;
                }
            }
            Err(_) => tracing::warn!("workflow command hosts remained shared at shutdown"),
        }
    }
}

fn effective_permission_mode(
    run: &StoredRun,
    current: crate::permission::PermissionMode,
) -> anyhow::Result<crate::permission::PermissionMode> {
    effective_permission_mode_for(
        current,
        run.graph
            .resolved_nodes
            .values()
            .filter_map(|node| match node {
                ResolvedNode::Agent(agent) => Some(agent.permission_ceiling.as_str()),
                ResolvedNode::Command(_) => None,
            }),
    )
}

fn effective_permission_mode_for<'a>(
    current: crate::permission::PermissionMode,
    frozen_ceilings: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<crate::permission::PermissionMode> {
    let mut effective = current;
    for frozen in frozen_ceilings {
        let frozen = frozen.parse().map_err(|error| {
            anyhow::anyhow!("frozen workflow permission ceiling is invalid: {error}")
        })?;
        effective = crate::app::agent_binding::narrower_permission_mode(frozen, effective);
    }
    Ok(effective)
}

#[allow(dead_code)]
pub(super) async fn execute_tool_run(
    run: StoredRun,
    recovery: RecoveryDecision,
    config_path: Option<std::path::PathBuf>,
    context: &rho_sdk::tool::ToolContext,
) -> anyhow::Result<StoredRun> {
    let runtime = WorkflowRuntime::build(&run, config_path, context.child_approval_session())?;
    let runner = Arc::clone(&runtime.runner);
    let cancellation = runner.cancellation_request(run.manifest.run_id);
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let result = {
        let drive = runner.drive(run.manifest.run_id, recovery, Some(sender));
        tokio::pin!(drive);
        let mut cancellation_requested = false;
        loop {
            tokio::select! {
                biased;
                () = context.cancellation().cancelled(), if !cancellation_requested => {
                    if let Err(error) = cancellation.request() {
                        break Err(anyhow::Error::from(error));
                    }
                    cancellation_requested = true;
                }
                result = &mut drive => {
                    while let Ok(event) = events.try_recv() {
                        let _ = context
                            .progress()
                            .send(rho_sdk::tool::ToolProgress::message(event.message()))
                            .await;
                    }
                    break result.map_err(anyhow::Error::from);
                }
                event = events.recv() => {
                    if let Some(event) = event {
                        let _ = context
                            .progress()
                            .send(rho_sdk::tool::ToolProgress::message(event.message()))
                            .await;
                    }
                }
            }
        }
    };
    drop(runner);
    runtime.shutdown().await;
    result
}

async fn drive_with_stream(
    runner: Arc<WorkflowRunner>,
    run: &StoredRun,
    recovery: RecoveryDecision,
    presentation: RuntimePresentation,
) -> anyhow::Result<bool> {
    let cancellation = runner.cancellation_request(run.manifest.run_id);
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
    let presenter = tokio::spawn(present_runtime_events(
        event_rx,
        presentation,
        run.manifest.run_id,
    ));
    let interrupted = {
        let drive = runner.drive(run.manifest.run_id, recovery, Some(event_tx));
        tokio::pin!(drive);
        tokio::select! {
            result = drive.as_mut() => {
                result?;
                false
            }
            result = workflow_shutdown_signal() => {
                result?;
                cancellation.request()?;
                drive.as_mut().await?;
                true
            }
        }
    };
    presenter
        .await
        .map_err(|error| anyhow::anyhow!("workflow event presenter failed: {error}"))??;
    Ok(interrupted)
}

struct RunnerTuiAdapter {
    runner: Arc<WorkflowRunner>,
    rho_home: std::path::PathBuf,
    run_id: RunId,
    initial: WorkflowSnapshot,
    events: tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
    worker: Option<tokio::task::JoinHandle<Result<StoredRun, RuntimeError>>>,
}

impl RunnerTuiAdapter {
    fn start(
        runner: Arc<WorkflowRunner>,
        rho_home: std::path::PathBuf,
        run: StoredRun,
        recovery: RecoveryDecision,
    ) -> Self {
        let run_id = run.manifest.run_id;
        let initial = tui_snapshot(&run);
        let (sender, events) = tokio::sync::mpsc::unbounded_channel();
        let worker_runner = Arc::clone(&runner);
        let worker =
            tokio::spawn(async move { worker_runner.drive(run_id, recovery, Some(sender)).await });
        Self {
            runner,
            rho_home,
            run_id,
            initial,
            events,
            worker: Some(worker),
        }
    }

    fn load_snapshot(&self) -> anyhow::Result<WorkflowSnapshot> {
        let store = WorkflowStore::new(&self.rho_home)?;
        Ok(tui_snapshot(&store.load_run(self.run_id)?))
    }

    async fn finish_worker(&mut self) -> anyhow::Result<()> {
        if let Some(worker) = self.worker.take() {
            worker
                .await
                .map_err(|error| anyhow::anyhow!("workflow runner task failed: {error}"))??;
        }
        Ok(())
    }
}

impl WorkflowEventAdapter for RunnerTuiAdapter {
    fn session(&self) -> WorkflowSession {
        WorkflowSession::Owner
    }

    fn initial_snapshot(&self) -> WorkflowSnapshot {
        self.initial.clone()
    }

    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<TuiEvent>>> + Send + '_>> {
        Box::pin(async move {
            match self.events.recv().await {
                Some(RuntimeEvent::NodeProgress {
                    node,
                    attempt,
                    message,
                    detail,
                    completed,
                    total,
                }) => Ok(Some(TuiEvent::Progress {
                    node,
                    progress: WorkflowProgress {
                        attempt,
                        completed,
                        total,
                        message,
                        detail,
                    },
                })),
                Some(_) => self
                    .load_snapshot()
                    .map(|snapshot| Some(TuiEvent::Snapshot(snapshot))),
                None => {
                    self.finish_worker().await?;
                    Ok(None)
                }
            }
        })
    }

    fn send(
        &mut self,
        action: WorkflowAction,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            match action {
                WorkflowAction::Cancel => {
                    self.runner.cancellation_request(self.run_id).request()?;
                }
                WorkflowAction::ConfirmPlan | WorkflowAction::ConfirmResume => {
                    anyhow::bail!("the workflow plan was already confirmed")
                }
            }
            Ok(())
        })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.runner.cancellation_request(self.run_id).request()?;
            self.finish_worker().await
        })
    }
}

/// Poll interval for read-only watch of a durable run snapshot.
const WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Opens the workflow DAG screen in read-only watch mode for an existing run.
pub(crate) async fn watch_run(run: StoredRun) -> anyhow::Result<()> {
    let adapter = WatchAdapter::new(crate::paths::rho_dir()?, run)?;
    crate::tui::workflow::run(Box::new(adapter)).await?;
    Ok(())
}

struct WatchAdapter {
    store: WorkflowStore,
    rho_home: std::path::PathBuf,
    run_id: RunId,
    initial: WorkflowSnapshot,
    last_revision: u64,
    interval: tokio::time::Interval,
}

impl WatchAdapter {
    fn new(rho_home: std::path::PathBuf, run: StoredRun) -> anyhow::Result<Self> {
        let run_id = run.manifest.run_id;
        let last_revision = run.state.state.revision;
        let mut interval = tokio::time::interval(WATCH_POLL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Ok(Self {
            store: WorkflowStore::new(&rho_home)?,
            rho_home,
            run_id,
            initial: tui_snapshot(&run),
            last_revision,
            interval,
        })
    }

    /// Cheap poll: read revision only. Full load happens on change.
    fn load_if_changed(&self) -> anyhow::Result<Option<(WorkflowSnapshot, u64)>> {
        let revision = self.store.read_run_revision(self.run_id)?;
        if revision == self.last_revision {
            return Ok(None);
        }
        let run = self.store.load_run(self.run_id)?;
        let revision = run.state.state.revision;
        Ok(Some((tui_snapshot(&run), revision)))
    }
}

impl WorkflowEventAdapter for WatchAdapter {
    fn session(&self) -> WorkflowSession {
        WorkflowSession::Watcher
    }

    fn initial_snapshot(&self) -> WorkflowSnapshot {
        self.initial.clone()
    }

    fn next_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<TuiEvent>>> + Send + '_>> {
        Box::pin(async move {
            loop {
                self.interval.tick().await;
                if let Some((snapshot, revision)) = self.load_if_changed()? {
                    self.last_revision = revision;
                    return Ok(Some(TuiEvent::Snapshot(snapshot)));
                }
            }
        })
    }

    fn send(
        &mut self,
        action: WorkflowAction,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            match action {
                WorkflowAction::Cancel => {
                    let lifecycle = self.store.read_run_lifecycle(self.run_id)?;
                    super::request_cancellation(&self.rho_home, self.run_id, lifecycle).await?;
                }
                WorkflowAction::ConfirmPlan | WorkflowAction::ConfirmResume => {
                    anyhow::bail!("watch mode cannot start or resume a plan")
                }
            }
            Ok(())
        })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }
}

fn tui_snapshot(run: &StoredRun) -> WorkflowSnapshot {
    let state = &run.state.state;
    let nodes = run
        .graph
        .graph
        .nodes
        .iter()
        .map(|(id, node)| {
            let node_state = state.nodes[id].clone();
            let current_attempt = match node_state {
                NodeState::Running { attempt } => Some(attempt),
                _ => None,
            };
            let execution = match &run.graph.resolved_nodes[id] {
                ResolvedNode::Agent(agent) => ExecutionMetadata::Agent {
                    name: agent.agent_id.clone(),
                    runtime: agent.runtime,
                    provider: agent.provider.clone(),
                    model: agent.model.clone(),
                },
                ResolvedNode::Command(command) => ExecutionMetadata::Command {
                    executable: command.executable.clone(),
                    cwd: command.cwd.clone(),
                    shell: matches!(
                        node.execution,
                        NodeExecution::Command(crate::workflow::CommandNode::Shell { .. })
                    ),
                },
            };
            WorkflowNodeSnapshot {
                id: id.clone(),
                display_name: node.display_name.clone(),
                dependencies: node.needs.clone(),
                access: node.access,
                execution,
                work: node_work_summary(&node.execution),
                state: node_state.clone(),
                current_attempt,
                command_exit: state.command_exits.get(id).cloned(),
                validated_output: state.outputs.get(id).cloned(),
                artifacts: durable_artifacts_for_node(state, id),
                terminal_reason: terminal_reason(&node_state),
            }
        })
        .collect();
    let lifecycle = state.lifecycle;
    WorkflowSnapshot {
        workflow_name: run.graph.graph.name.to_string(),
        plan_id: run.manifest.plan_id,
        run_id: Some(run.manifest.run_id),
        graph_digest: run.manifest.graph_digest.clone(),
        sources: SourceDigestSummary {
            source_count: run.graph.sources.modules.len(),
            digest: source_digest(run),
        },
        approval: PlanApprovalState::Approved,
        lifecycle,
        outcome: derive_workflow_outcome(&run.graph, state),
        nodes,
        cancellation: if state.cancellation_requested {
            if lifecycle == RunLifecycle::Completed {
                CancellationState::Saved
            } else {
                CancellationState::Requested
            }
        } else {
            CancellationState::NotRequested
        },
        recovery_requirement: None,
        exit_is_safe: matches!(
            lifecycle,
            RunLifecycle::Completed | RunLifecycle::NeedsRecovery
        ),
    }
}

fn durable_artifacts_for_node(
    state: &crate::workflow::WorkflowState,
    id: &crate::workflow::NodeId,
) -> Vec<crate::tui::workflow::ArtifactReference> {
    state
        .completions
        .get(id)
        .into_iter()
        .flat_map(|completion| completion.artifacts.iter())
        .map(|(kind, artifact)| crate::tui::workflow::ArtifactReference {
            kind,
            artifact: artifact.clone(),
        })
        .collect()
}

fn node_work_summary(execution: &NodeExecution) -> String {
    match execution {
        NodeExecution::Agent(agent) => {
            let preview = template_preview(&agent.prompt);
            if preview.is_empty() {
                format!("agent {}", agent.agent)
            } else {
                preview
            }
        }
        NodeExecution::Command(CommandNode::Direct {
            executable,
            arguments,
            ..
        }) => {
            let exe = std::path::Path::new(executable)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(executable);
            if arguments.is_empty() {
                format!("run {exe}")
            } else {
                let args = arguments
                    .iter()
                    .map(template_preview)
                    .collect::<Vec<_>>()
                    .join(" ");
                truncate_chars(&format!("run {exe} {args}"), 160)
            }
        }
        NodeExecution::Command(CommandNode::Shell { command, .. }) => {
            truncate_chars(&format!("shell: {command}"), 160)
        }
    }
}

fn template_preview(template: &Template) -> String {
    let mut out = String::new();
    for part in &template.0 {
        match part {
            TemplatePart::Literal { value } => out.push_str(value),
            TemplatePart::Output { reference } => {
                let path = if reference.path.0.is_empty() {
                    String::new()
                } else {
                    format!(".{}", reference.path.0.join("."))
                };
                out.push_str(&format!("{{{{{node}{path}}}}}", node = reference.node));
            }
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, 160)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn source_digest(run: &StoredRun) -> Digest {
    let mut hash = Sha256::new();
    for (label, source) in &run.graph.sources.modules {
        hash.update(label.as_bytes());
        hash.update([0]);
        hash.update(source.digest.0.as_bytes());
        hash.update([0]);
    }
    Digest(format!("sha256:{:x}", hash.finalize()))
}

fn terminal_reason(state: &NodeState) -> Option<TerminalReason> {
    let NodeState::Terminal { outcome } = state else {
        return None;
    };
    match outcome {
        NodeTerminalState::Success | NodeTerminalState::Skipped => None,
        NodeTerminalState::Failure => Some(TerminalReason::Failure("node failed".into())),
        NodeTerminalState::Denial => Some(TerminalReason::Denial("node was denied".into())),
        NodeTerminalState::Cancellation => {
            Some(TerminalReason::Cancellation("node was cancelled".into()))
        }
        NodeTerminalState::Blocked => Some(TerminalReason::Blocked("node was blocked".into())),
    }
}

async fn present_runtime_events(
    mut events: tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
    presentation: RuntimePresentation,
    run_id: crate::workflow::RunId,
) -> anyhow::Result<()> {
    let mut sequence = 0_u64;
    while let Some(event) = events.recv().await {
        sequence = sequence.saturating_add(1);
        match presentation {
            RuntimePresentation::Text => println!("{}", event.message()),
            RuntimePresentation::Jsonl => {
                let value = runtime_event_json(sequence, run_id, &event);
                write_json_document(&value)?;
            }
        }
    }
    Ok(())
}

fn runtime_event_json(
    sequence: u64,
    run_id: crate::workflow::RunId,
    event: &RuntimeEvent,
) -> serde_json::Value {
    let mut value = serde_json::to_value(event).expect("RuntimeEvent serializes");
    let object = value
        .as_object_mut()
        .expect("RuntimeEvent serializes to an object");
    object.insert("version".into(), WORKFLOW_WIRE_VERSION.into());
    object.insert("sequence".into(), sequence.into());
    object.insert("run_id".into(), run_id.to_string().into());
    value
}

#[cfg(unix)]
async fn workflow_shutdown_signal() -> io::Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => Ok(()),
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn workflow_shutdown_signal() -> io::Result<()> {
    tokio::signal::ctrl_c().await
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
