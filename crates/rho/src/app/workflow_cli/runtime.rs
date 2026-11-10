//! Workflow run composition: hosts, execute/spawn, and drive selection.

use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::Arc,
};

use rho_sdk::{
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, ApprovalSession,
    CapabilityOperation, ProviderRequestUsageRecording, ToolHost, Workspace,
};

use crate::{
    app::{
        agent_executor::AgentExecutor,
        bootstrap::absolute_config_path,
        config_repository::ConfigRepository,
        policy::AppPolicy,
        session_runtime::UnattendedApprovals,
        subagent_host_input::SubagentHostInputBridge,
        subagent_pool::{StartedWorkflow, SubagentPool, WorkflowEnd},
        workflow_runtime::{
            CommandHostFactory, RecoveryDecision, RuntimeError, RuntimeSecurity,
            WorkflowAgentExecutor, WorkflowCommandExecutor, WorkflowNodeExecutor, WorkflowRunner,
        },
    },
    cli::WorkflowRunFormat,
    config::Config,
    permission::{remember_allowed_workspace_writes, PermissionMode, SessionWriteLog},
    permission_classifier_handler::ClassifierApprovalHandler,
    session_environment::SessionEnvironment,
    tools::workflow_tracker::WorkflowRunTracker,
    workflow::{ResolvedNode, RunId, StoredRun, WorkflowOutcome},
};

use super::{
    runtime_present::{drive_with_stream, RuntimePresentation},
    runtime_tui::RunnerTuiAdapter,
};

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

pub(super) enum WorkflowApprovalMode {
    InteractiveTerminal {
        can_prompt: bool,
        usage_recording: ProviderRequestUsageRecording,
    },
    NonInteractive {
        usage_recording: ProviderRequestUsageRecording,
    },
}

impl WorkflowApprovalMode {
    fn interactive_terminal(
        can_prompt: bool,
        usage_recording: ProviderRequestUsageRecording,
    ) -> Self {
        Self::InteractiveTerminal {
            can_prompt,
            usage_recording,
        }
    }

    fn non_interactive(usage_recording: ProviderRequestUsageRecording) -> Self {
        Self::NonInteractive { usage_recording }
    }

    fn can_prompt(&self) -> bool {
        matches!(
            self,
            Self::InteractiveTerminal {
                can_prompt: true,
                ..
            }
        )
    }

    fn usage_recording(&self) -> ProviderRequestUsageRecording {
        match self {
            Self::InteractiveTerminal {
                usage_recording, ..
            }
            | Self::NonInteractive { usage_recording } => usage_recording.clone(),
        }
    }
}

struct WorkflowApprovalChannel {
    session: ApprovalSession,
    classifier: Option<Arc<ClassifierApprovalHandler>>,
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
    eprint!("allow [o]nce, allow for [s]ession, or [d]eny? ");
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

fn workflow_approval_channel(
    config: &Config,
    permission_mode: PermissionMode,
    workspace_path: PathBuf,
    approval_mode: WorkflowApprovalMode,
    session_writes: SessionWriteLog,
    credentials: Arc<dyn rho_providers::credentials::CredentialStore>,
) -> anyhow::Result<WorkflowApprovalChannel> {
    let (handler, classifier): (Arc<dyn ApprovalHandler>, _) = match permission_mode {
        PermissionMode::Auto => {
            crate::app::session_runtime::ensure_auto_classifier_model(
                config,
                credentials.as_ref(),
            )?;
            let human = approval_mode.can_prompt().then(|| {
                Arc::new(TerminalWorkflowApprovals { interactive: true })
                    as Arc<dyn ApprovalHandler>
            });
            let classifier = ClassifierApprovalHandler::shared(
                config.clone(),
                workspace_path,
                approval_mode.usage_recording(),
                credentials,
                human,
                Some(session_writes.clone()),
            );
            (classifier.clone(), Some(classifier))
        }
        PermissionMode::AllowEdits
        | PermissionMode::Supervised
        | PermissionMode::Bypass
        | PermissionMode::Plan => (
            Arc::new(TerminalWorkflowApprovals {
                interactive: approval_mode.can_prompt(),
            }),
            None,
        ),
    };
    let handler = match permission_mode {
        PermissionMode::AllowEdits => remember_allowed_workspace_writes(
            handler,
            session_writes,
            crate::permission::WriteAuthority::Human,
        ),
        _ => handler,
    };
    Ok(WorkflowApprovalChannel {
        session: ApprovalSession::from_shared(handler),
        classifier,
    })
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
            .workspace_policy(self.policy.clone())
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
    let interactive_input = crate::stdio::stdin_is_terminal();
    let interactive_terminal = interactive_input && io::stdout().is_terminal();
    let interactive_display = io::stderr().is_terminal();
    let presentation = match output {
        Some(WorkflowRunFormat::Jsonl) => RuntimePresentation::Jsonl,
        Some(WorkflowRunFormat::Text) | None => RuntimePresentation::Text,
    };
    let rho_home = crate::paths::rho_dir()?;
    let approval_mode = WorkflowApprovalMode::interactive_terminal(
        interactive_input && interactive_display,
        crate::usage::default_recording().await,
    );
    let runtime = WorkflowRuntime::build(
        &run,
        &std::env::current_dir()?,
        config_path,
        approval_mode,
        SubagentPool::Own,
        // A run driven from the terminal, `rho workflow run` or `resume`,
        // runs in this process's environment (#1659).
        SessionEnvironment::default(),
    )?;
    let custom_providers = runtime.custom_providers.clone();
    rho_providers::provider::scope_custom_openai_compatible_providers(
        custom_providers,
        async move {
            let use_tui = output.is_none()
                && interactive_terminal
                && interactive_display
                && !matches!(
                    runtime.permission_mode,
                    crate::permission::PermissionMode::AllowEdits
                        | crate::permission::PermissionMode::Supervised
                );
            let runner = Arc::clone(&runtime.runner);
            let execution = if use_tui {
                let adapter =
                    RunnerTuiAdapter::start(Arc::clone(&runner), rho_home, run.clone(), recovery)?;
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
        },
    )
    .await
}

/// Where a background run is driven from, and who hears how it went.
pub(crate) struct BackgroundRun {
    /// The workspace the run was planned in.
    pub(crate) cwd: PathBuf,
    /// The config the run settles its agents against; `None` for the
    /// default config.
    pub(crate) config_path: Option<PathBuf>,
    /// The starting session's pool, which the run's agent nodes share. A
    /// shared pool's observer hears of the run and says where its nodes run.
    pub(crate) pool: SubagentPool,
    /// Delivers the run's completion to the starting session.
    pub(crate) tracker: WorkflowRunTracker,
    /// The environment the run's hooks, agent nodes, and commands run in
    /// (#1659), with its project trust (#1779): the starting session's when
    /// it started or resumed the run, the snapshot planning or the resume
    /// recheck used. Later prompts do not change it. It is never saved, so
    /// a run resumed later takes the environment of whatever resumes it.
    pub(crate) environment: SessionEnvironment,
}

/// Starts a workflow run on a detached task and returns immediately.
///
/// The caller keeps chatting or returns a tool result while the run continues.
/// Inspect progress with status; stop it with cancel. The owner process must
/// stay alive for the run to make progress. The starting session receives a
/// completion notification through `background.tracker` after the driver
/// finishes.
pub(crate) async fn spawn_background_run(
    run: StoredRun,
    recovery: RecoveryDecision,
    background: BackgroundRun,
) -> anyhow::Result<StoredRun> {
    let BackgroundRun {
        cwd,
        config_path,
        pool,
        tracker,
        environment,
    } = background;
    let run_id = run.manifest.run_id;
    tracker.update_from_stored(&run);
    let (nodes, end) = pool.host_workflow(StartedWorkflow {
        run_id,
        workspace: cwd.clone(),
    });
    let runtime = match WorkflowRuntime::build(
        &run,
        &cwd,
        config_path,
        WorkflowApprovalMode::non_interactive(crate::usage::default_recording().await),
        nodes,
        environment,
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            // The tracker's notice reaches the model, so it keeps the top
            // message; the full chain, which can name paths, goes only to
            // the host following the run.
            tracker.mark_failed(&run_id.to_string(), error.to_string());
            if let Some(end) = end {
                // The host may already have stopped following the run.
                let _ = end.send(WorkflowEnd::Failed(format!("{error:#}")));
            }
            return Err(error);
        }
    };
    let runner = Arc::clone(&runtime.runner);
    let custom_providers = runtime.custom_providers.clone();
    tokio::spawn(async move {
        rho_providers::provider::scope_custom_openai_compatible_providers(
            custom_providers,
            async move {
                let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
                let forward_events = async {
                    while let Some(event) = event_rx.recv().await {
                        tracker.record_event(&run_id.to_string(), &event);
                    }
                };
                let (result, ()) = tokio::join!(
                    runner.drive(run_id, recovery, Some(event_tx)),
                    forward_events,
                );
                drop(runner);
                runtime.shutdown().await;
                match &result {
                    Ok(final_run) => tracker.mark_finished_from_stored(final_run),
                    Err(error) => tracker.mark_failed(&run_id.to_string(), error.to_string()),
                }
                let ended = drive_end(&result);
                log_drive_end(run_id, &ended);
                if let Some(end) = end {
                    // The host may already have stopped following the run.
                    let _ = end.send(ended);
                }
            },
        )
        .await;
    });
    // Return the pre-spawn snapshot immediately. Progress is eventually consistent
    // via status / watch / automatic completion notification.
    Ok(run)
}

/// Logs how background run `run_id` ended: a run that failed, was denied or
/// blocked logs as failed with its outcome, not as completed (#1736).
fn log_drive_end(run_id: RunId, end: &WorkflowEnd) {
    match end {
        WorkflowEnd::Completed => tracing::info!(%run_id, "background workflow completed"),
        WorkflowEnd::Cancelled => tracing::info!(%run_id, "background workflow cancelled"),
        WorkflowEnd::Failed(reason) => {
            tracing::warn!(%run_id, error = %reason, "background workflow failed")
        }
    }
}

/// How a drive that returned `result` ended, for the host following it.
fn drive_end(result: &Result<StoredRun, RuntimeError>) -> WorkflowEnd {
    let run = match result {
        Ok(run) => run,
        Err(error) => return WorkflowEnd::Failed(error.to_string()),
    };
    let state = &run.state.state;
    match state.outcome() {
        Some(WorkflowOutcome::Success) => WorkflowEnd::Completed,
        Some(WorkflowOutcome::Cancellation) => WorkflowEnd::Cancelled,
        Some(
            outcome @ (WorkflowOutcome::Failure
            | WorkflowOutcome::Denial
            | WorkflowOutcome::Blocked),
        ) => WorkflowEnd::Failed(format!("the workflow run ended with {}", outcome.as_str())),
        None => WorkflowEnd::Failed(format!(
            "the workflow run stopped in state {}",
            state.lifecycle.as_str()
        )),
    }
}

struct WorkflowRuntime {
    runner: Arc<WorkflowRunner>,
    command_executor:
        Arc<dyn WorkflowNodeExecutor<crate::app::workflow_runtime::CommandInvocation>>,
    hosts: Arc<WorkflowCommandHosts>,
    permission_mode: crate::permission::PermissionMode,
    custom_providers: std::sync::Arc<[String]>,
}

impl WorkflowRuntime {
    /// The runtime that drives `run`, planned in `cwd`, its agent nodes
    /// taking capacity from `nodes` and all its children running in
    /// `environment`.
    fn build(
        run: &StoredRun,
        cwd: &std::path::Path,
        config_path: Option<std::path::PathBuf>,
        approval_mode: WorkflowApprovalMode,
        nodes: SubagentPool,
        // The run's own handle, which nothing replaces: its command
        // executables, their `#!/usr/bin/env` interpreters, and CLI agent
        // binaries were frozen on one `PATH`, and must verify on that `PATH`
        // when each node starts, not on whichever client prompted the
        // session since (#1659).
        environment: SessionEnvironment,
    ) -> anyhow::Result<Self> {
        let cwd = cwd.canonicalize()?;
        let repository = ConfigRepository::new(config_path);
        let config_path = absolute_config_path(&repository)?;
        let mut config = repository.load()?;
        let custom_providers = config.providers.intern_names()?;
        let _scope =
            rho_providers::provider::CustomProviderThreadScope::enter(custom_providers.clone());
        let permission_mode = effective_permission_mode(run, config.permission_mode)?;
        config.permission_mode = permission_mode;
        let session_writes = SessionWriteLog::default();
        let approvals = workflow_approval_channel(
            &config,
            permission_mode,
            cwd.clone(),
            approval_mode,
            session_writes.clone(),
            environment.credentials(),
        )?;
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
        let hooks = crate::hooks::start_for_cwd(&cwd, environment.clone());
        let hook_engine = hooks.as_ref().map(|pipeline| Arc::clone(pipeline.engine()));
        let command_classifier = approvals
            .classifier
            .as_ref()
            .map(ClassifierApprovalHandler::isolate);
        let command_approvals = match &command_classifier {
            // Commands get an isolated streak counter so agent denials cannot
            // escalate command approvals (and vice versa).
            Some(handler) => ApprovalSession::from_shared(handler.clone()),
            None => approvals.session.clone(),
        };
        let hosts = Arc::new(WorkflowCommandHosts {
            workspace,
            policy: AppPolicy::for_mode(
                permission_mode,
                session_writes,
                environment.current().path(),
            ),
            approvals: command_approvals,
            hooks,
        });
        let agent_approvals = match approvals.classifier.clone() {
            Some(classifier) => UnattendedApprovals::ClassifierTemplate(classifier),
            None => UnattendedApprovals::Inherited(approvals.session),
        };
        let (concurrency, observer) = nodes.into_parts(config.agent_concurrency);
        let app_agent_executor = Arc::new(
            AgentExecutor::new(
                config.clone(),
                config_path,
                cwd.clone(),
                concurrency,
                SubagentHostInputBridge::new(),
                crate::app::subagent_messaging::SubagentNoticeBridge::new(),
            )
            .with_approvals(agent_approvals)
            // Agent nodes run in the run's environment.
            .with_environment(environment.clone()),
        );
        let agent_executor: Arc<
            dyn WorkflowNodeExecutor<crate::app::workflow_runtime::AgentInvocation>,
        > = Arc::new(WorkflowAgentExecutor::new(app_agent_executor, observer));
        let command_executor: Arc<
            dyn WorkflowNodeExecutor<crate::app::workflow_runtime::CommandInvocation>,
        > = Arc::new(WorkflowCommandExecutor::new(
            environment.process(),
            Arc::clone(&hosts) as Arc<dyn CommandHostFactory>,
        ));
        let security = RuntimeSecurity {
            project_trusted: environment.project_trust().agents.is_trusted(),
            project_plugins_trusted: environment.project_trust().plugins.is_trusted(),
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
        runner = runner.with_custom_providers(custom_providers.clone());
        Ok(Self {
            runner: Arc::new(runner),
            command_executor,
            hosts,
            permission_mode,
            custom_providers,
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

pub(super) fn effective_permission_mode_for<'a>(
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

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
