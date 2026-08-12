//! Workflow run composition: hosts, execute/spawn, and drive selection.

use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::Arc,
};

use rho_sdk::{
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, ApprovalSession,
    CapabilityOperation, ProcessEnvironment, ProviderRequestUsageRecording, ToolHost, Workspace,
};

use crate::{
    app::{
        agent_executor::AgentExecutor,
        automation::ensure_headless_auto_classifier_model,
        bootstrap::absolute_config_path,
        config_repository::ConfigRepository,
        policy::AppPolicy,
        subagent_host_input::SubagentHostInputBridge,
        workflow_runtime::{
            CommandHostFactory, RecoveryDecision, RuntimeError, RuntimeSecurity,
            WorkflowAgentExecutor, WorkflowCommandExecutor, WorkflowNodeExecutor, WorkflowRunner,
        },
    },
    cli::WorkflowRunFormat,
    config::Config,
    permission::PermissionMode,
    permission_classifier_handler::ClassifierApprovalHandler,
    workflow::{ResolvedNode, StoredRun},
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
) -> anyhow::Result<WorkflowApprovalChannel> {
    match permission_mode {
        PermissionMode::Auto => {
            ensure_headless_auto_classifier_model(config)?;
            let human = approval_mode.can_prompt().then(|| {
                Arc::new(TerminalWorkflowApprovals { interactive: true })
                    as Arc<dyn ApprovalHandler>
            });
            let classifier = ClassifierApprovalHandler::shared(
                config.clone(),
                workspace_path,
                approval_mode.usage_recording(),
                human,
            );
            let handler: Arc<dyn ApprovalHandler> = classifier.clone();
            Ok(WorkflowApprovalChannel {
                session: ApprovalSession::from_shared(handler),
                classifier: Some(classifier),
            })
        }
        PermissionMode::Supervised => Ok(WorkflowApprovalChannel {
            session: ApprovalSession::new(TerminalWorkflowApprovals {
                interactive: approval_mode.can_prompt(),
            }),
            classifier: None,
        }),
        PermissionMode::Bypass | PermissionMode::Plan => Ok(WorkflowApprovalChannel {
            session: ApprovalSession::new(TerminalWorkflowApprovals {
                interactive: approval_mode.can_prompt(),
            }),
            classifier: None,
        }),
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
    let approval_mode = WorkflowApprovalMode::interactive_terminal(
        interactive_input && interactive_display,
        crate::usage::default_recording().await,
    );
    let runtime = WorkflowRuntime::build(&run, config_path, approval_mode)?;
    let use_tui = output.is_none()
        && interactive_terminal
        && interactive_display
        && runtime.permission_mode != crate::permission::PermissionMode::Supervised;
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
}

/// Starts a workflow run on a detached task and returns immediately.
///
/// The caller keeps chatting or returns a tool result while the run continues.
/// Inspect progress with status; stop it with cancel. The owner process must
/// stay alive for the run to make progress. When `tracker` is set, the parent
/// session receives a completion notification after the driver finishes.
pub(crate) async fn spawn_background_run(
    run: StoredRun,
    recovery: RecoveryDecision,
    config_path: Option<std::path::PathBuf>,
    tracker: Option<crate::tools::workflow_tracker::WorkflowRunTracker>,
) -> anyhow::Result<StoredRun> {
    let run_id = run.manifest.run_id;
    let runtime = WorkflowRuntime::build(
        &run,
        config_path,
        WorkflowApprovalMode::non_interactive(crate::usage::default_recording().await),
    )?;
    let runner = Arc::clone(&runtime.runner);
    tokio::spawn(async move {
        let result = runner.drive(run_id, recovery, None).await;
        drop(runner);
        runtime.shutdown().await;
        if let Some(tracker) = tracker {
            match crate::paths::rho_dir() {
                Ok(home) => match crate::workflow::WorkflowStore::new(&home) {
                    Ok(store) => match store.load_run(run_id) {
                        Ok(final_run) => tracker.mark_finished_from_stored(&final_run),
                        Err(error) => match &result {
                            Ok(_) => tracker.mark_failed(
                                &run_id.to_string(),
                                format!(
                                    "workflow finished but status could not be loaded: {error}"
                                ),
                            ),
                            Err(drive_error) => tracker.mark_failed(
                                &run_id.to_string(),
                                format!("{drive_error}; status load failed: {error}"),
                            ),
                        },
                    },
                    Err(error) => match &result {
                        Ok(_) => tracker.mark_failed(
                            &run_id.to_string(),
                            format!("workflow finished but store could not be opened: {error}"),
                        ),
                        Err(drive_error) => tracker.mark_failed(
                            &run_id.to_string(),
                            format!("{drive_error}; store open failed: {error}"),
                        ),
                    },
                },
                Err(error) => match &result {
                    Ok(_) => tracker.mark_failed(
                        &run_id.to_string(),
                        format!("workflow finished but rho home is unavailable: {error}"),
                    ),
                    Err(drive_error) => tracker.mark_failed(
                        &run_id.to_string(),
                        format!("{drive_error}; rho home unavailable: {error}"),
                    ),
                },
            }
        }
        match result {
            Ok(_) => tracing::info!(%run_id, "background workflow completed"),
            Err(error) => {
                tracing::warn!(%run_id, error = %error, "background workflow failed")
            }
        }
    });
    // Return the pre-spawn snapshot immediately. Progress is eventually consistent
    // via status / watch / automatic completion notification.
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
        approval_mode: WorkflowApprovalMode,
    ) -> anyhow::Result<Self> {
        let cwd = std::env::current_dir()?.canonicalize()?;
        let repository = ConfigRepository::new(config_path);
        let config_path = absolute_config_path(&repository)?;
        let mut config = repository.load()?;
        let permission_mode = effective_permission_mode(run, config.permission_mode)?;
        config.permission_mode = permission_mode;
        let approvals =
            workflow_approval_channel(&config, permission_mode, cwd.clone(), approval_mode)?;
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
        let command_classifier = approvals
            .classifier
            .as_ref()
            .map(ClassifierApprovalHandler::isolate);
        let command_approvals = match &command_classifier {
            // Commands get an isolated streak counter so agent denials cannot
            // escalate command approvals (and vice versa).
            Some(handler) => {
                let erased: Arc<dyn ApprovalHandler> = handler.clone();
                ApprovalSession::from_shared(erased)
            }
            None => approvals.session.clone(),
        };
        let hosts = Arc::new(WorkflowCommandHosts {
            workspace,
            policy: AppPolicy::for_mode(permission_mode),
            approvals: command_approvals,
            hooks,
        });
        let process_environment = ProcessEnvironment::inherit_except(
            rho_providers::credential_env_vars().iter().copied(),
        );
        let app_agent_executor = Arc::new(match approvals.classifier.clone() {
            Some(classifier) => AgentExecutor::new(
                config.clone(),
                config_path,
                cwd.clone(),
                SubagentHostInputBridge::new(),
                crate::app::subagent_messaging::SubagentNoticeBridge::new(),
            )
            .with_classifier_template(classifier),
            None => AgentExecutor::new(
                config.clone(),
                config_path,
                cwd.clone(),
                SubagentHostInputBridge::new(),
                crate::app::subagent_messaging::SubagentNoticeBridge::new(),
            )
            .with_approval_session(approvals.session),
        });
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
