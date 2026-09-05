use std::{path::PathBuf, sync::Arc};

use {
    crate::agent::{AgentCapabilities, AgentDefinition, ToolCapability},
    crate::cli::OutputFormat,
    crate::config::Config,
    crate::diagnostics::RuntimeDiagnostics,
    crate::herdr::HerdrReporter,
    crate::permission_classifier_handler::ClassifierApprovalHandler,
    crate::subagent::{self, RunState, RunStatus},
    rho_tools::cancellation::RunCancellation,
};

use super::{
    agent_binding::{AgentBinder, AgentInvocation, AgentRole},
    agent_concurrency::AgentConcurrency,
    automation::{self, RunReporter},
    subagent_host_input::{SubagentHostInputBridge, SubagentHostInputResponder},
    subagent_messaging::{
        NoticePostError, NoticePoster, SteeringSlot, SubagentNotice, SubagentNoticeBridge,
        ValidatedMessage,
    },
};

#[derive(Clone)]
pub(crate) struct AgentExecutor {
    config: Arc<std::sync::RwLock<Config>>,
    config_path: PathBuf,
    cwd: PathBuf,
    /// Live-resizable global + nested Claude capacity for delegated runs.
    concurrency: AgentConcurrency,
    host_input: SubagentHostInputBridge,
    notices: SubagentNoticeBridge,
    approval_session: Option<rho_sdk::ApprovalSession>,
    approval_classifier: Option<Arc<ClassifierApprovalHandler>>,
}

pub(crate) struct AgentLaunchRequest {
    pub(crate) definition: Arc<AgentDefinition>,
    pub(crate) prompt: String,
    pub(crate) run_id: String,
    pub(crate) parent_session_id: Option<rho_sdk::SessionId>,
    pub(crate) output_file: PathBuf,
}

/// Launch request whose agent definition was resolved and frozen at plan time.
pub(crate) struct FrozenAgentLaunchRequest {
    pub(crate) agent: crate::workflow::ResolvedAgent,
    pub(crate) prompt: String,
    pub(crate) run_id: String,
    pub(crate) output_file: PathBuf,
    pub(crate) hook_host_labels: rho_sdk::hooks::HookHostLabels,
}

/// How a live handle can accept parent plain-text messages.
#[derive(Clone, Debug)]
enum MessagingSupport {
    /// Rho runtime: steering port is published once the session starts.
    Rho { steering: SteeringSlot },
    /// Claude-cli runtime: stream-json stdin turns while the child is live.
    Claude {
        messages: crate::claude_runtime::messaging::ClaudeMessageHandle,
    },
    /// Cursor runtime: one prompt on stdin, then the process ends.
    Unsupported,
}

/// Exhaustive launch target after bind. The session task matches this instead
/// of probing Claude-then-Rho.
enum Launch {
    Rho(super::agent_binding::BoundAgent),
    ClaudeCli(crate::claude_runtime::session::ClaudeSessionRequest),
    Cursor(crate::cursor_runtime::session::CursorSessionRequest),
}

/// Messaging ports created with the run handle, before the session task starts.
struct RuntimeMessagingPorts {
    messaging: MessagingSupport,
    steering_slot: Option<SteeringSlot>,
    claude_parent_rx: Option<crate::claude_runtime::messaging::ClaudeMessageInbox>,
}

impl MessagingSupport {
    /// Decides messaging support and the run's ports together, so the
    /// handle and session task cannot disagree about which path is live.
    fn for_runtime(runtime: &super::agent_binding::BoundRuntime) -> RuntimeMessagingPorts {
        match runtime {
            super::agent_binding::BoundRuntime::ClaudeCli { .. } => {
                let (handle, inbox) = crate::claude_runtime::messaging::message_channel();
                RuntimeMessagingPorts {
                    messaging: Self::Claude { messages: handle },
                    steering_slot: None,
                    claude_parent_rx: Some(inbox),
                }
            }
            super::agent_binding::BoundRuntime::Rho { .. } => {
                let steering = SteeringSlot::new();
                RuntimeMessagingPorts {
                    messaging: Self::Rho {
                        steering: steering.clone(),
                    },
                    steering_slot: Some(steering),
                    claude_parent_rx: None,
                }
            }
            super::agent_binding::BoundRuntime::Cursor { .. } => RuntimeMessagingPorts {
                messaging: Self::Unsupported,
                steering_slot: None,
                claude_parent_rx: None,
            },
        }
    }
}

#[derive(Clone)]
pub(crate) struct AgentRunHandle {
    cancellation: RunCancellation,
    status: tokio::sync::watch::Receiver<RunStatus>,
    completion: tokio::sync::watch::Receiver<bool>,
    messaging: MessagingSupport,
}

impl AgentRunHandle {
    pub(crate) fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub(crate) fn status(&self) -> RunStatus {
        self.status.borrow().clone()
    }

    pub(crate) fn is_complete(&self) -> bool {
        *self.completion.borrow()
    }

    pub(crate) async fn wait(&mut self) -> RunStatus {
        while !*self.completion.borrow() {
            if self.completion.changed().await.is_err() {
                break;
            }
        }
        self.status()
    }

    /// Clone of the live status watch used while the run is in flight.
    pub(crate) fn clone_status_watch(&self) -> tokio::sync::watch::Receiver<RunStatus> {
        self.status.clone()
    }

    /// Stages a parent message for the next Rho provider turn or Claude stdin turn.
    pub(crate) async fn message_from_parent(
        &self,
        message: &ValidatedMessage,
    ) -> anyhow::Result<()> {
        match &self.messaging {
            MessagingSupport::Rho { steering } => {
                if self.is_complete() {
                    anyhow::bail!("delegated run has already finished");
                }
                let Some(handle) = steering.handle() else {
                    anyhow::bail!(
                        "delegated run is still starting; wait until status is running, then message again"
                    );
                };
                handle
                    .steer(rho_sdk::UserInput::text(
                        super::subagent_messaging::parent_message_prompt(message),
                    ))
                    .await
                    .map_err(|error| anyhow::anyhow!("{error}"))
            }
            MessagingSupport::Claude { messages } => {
                if self.is_complete() {
                    anyhow::bail!("delegated run has already finished");
                }
                // Drain frames the body the same way Rho steering does.
                messages
                    .send(message.as_str().to_string())
                    .await
                    .map_err(|error| anyhow::anyhow!("{error}"))
            }
            MessagingSupport::Unsupported => anyhow::bail!(
                "cursor runs are process-per-turn and cannot accept messages; wait for completion"
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn completed_for_test(status: RunStatus) -> Self {
        let (_status_tx, status_rx) = tokio::sync::watch::channel(status);
        let (_completion_tx, completion_rx) = tokio::sync::watch::channel(true);
        let (messages, inbox) = crate::claude_runtime::messaging::message_channel();
        // Drop the inbox so late parent messages fail closed like a finished run.
        drop(inbox);
        Self {
            cancellation: RunCancellation::new(),
            status: status_rx,
            completion: completion_rx,
            messaging: MessagingSupport::Claude { messages },
        }
    }
}

impl AgentExecutor {
    pub(crate) fn new(
        config: Config,
        config_path: PathBuf,
        cwd: PathBuf,
        host_input: SubagentHostInputBridge,
        notices: SubagentNoticeBridge,
    ) -> Self {
        let concurrency = AgentConcurrency::from_config(config.agent_concurrency);
        Self {
            config: Arc::new(std::sync::RwLock::new(config)),
            config_path,
            cwd,
            concurrency,
            host_input,
            notices,
            approval_session: None,
            approval_classifier: None,
        }
    }

    /// Routes workflow-agent capability requests through the workflow run's
    /// host and exact-request approval memory.
    pub(crate) fn with_approval_session(mut self, session: rho_sdk::ApprovalSession) -> Self {
        self.approval_session = Some(session);
        self
    }

    /// Supplies a classifier template that each Rho agent run isolates so deny
    /// streaks stay per-run. History and cancellation arrive on each
    /// [`rho_sdk::ApprovalRequest::context`].
    pub(crate) fn with_classifier_template(
        mut self,
        classifier: Arc<ClassifierApprovalHandler>,
    ) -> Self {
        self.approval_classifier = Some(classifier);
        self
    }

    pub(crate) fn host_input(&self) -> &SubagentHostInputBridge {
        &self.host_input
    }

    pub(crate) fn notices(&self) -> &SubagentNoticeBridge {
        &self.notices
    }

    /// Atomically updates the parent session's provider selection inherited by
    /// future **Rho** runtime delegated agents.
    ///
    /// Claude-cli agents never read this snapshot for spawn: binding copies
    /// Claude model/tools/inherit from the definition only, byte-for-byte.
    pub(crate) fn update_selection(
        &self,
        provider: &str,
        model: &str,
        reasoning: rho_sdk::ReasoningLevel,
        auth: &str,
    ) {
        let mut config = self.config.write().expect("delegated config lock");
        config.provider = provider.to_string();
        config.model = model.to_string();
        config.reasoning = reasoning;
        config.auth = auth.to_string();
    }

    pub(crate) fn update_permission_mode(&self, mode: crate::permission::PermissionMode) {
        self.config
            .write()
            .expect("delegated config lock")
            .permission_mode = mode;
    }

    pub(crate) fn concurrency(&self) -> AgentConcurrency {
        self.concurrency.clone()
    }

    #[cfg(test)]
    pub(crate) fn launch_permission_mode(&self) -> crate::permission::PermissionMode {
        self.config
            .read()
            .expect("delegated config lock")
            .permission_mode
    }

    pub(crate) fn spawn(&self, request: AgentLaunchRequest) -> anyhow::Result<AgentRunHandle> {
        let config = self.config.read().expect("delegated config lock").clone();
        let mut capabilities = AgentCapabilities::all_host_tools();
        // web_search is gated after bind against the agent provider/model.
        #[cfg(windows)]
        capabilities.remove(&ToolCapability::Bash);
        #[cfg(not(windows))]
        capabilities.remove(&ToolCapability::Powershell);
        // Delegated questionnaires route through the parent session. The parent
        // TUI can present them while a turn is running (foreground wait) or after
        // background dispatch, so availability is the live parent bridge only.
        let questionnaire_target = if delegated_questionnaire_available(
            request.parent_session_id.as_ref(),
            self.host_input.is_bound(),
        ) {
            request.parent_session_id.clone()
        } else {
            None
        };
        // Notices share the parent-session binding, not the questionnaire one.
        // They are non-blocking, so foreground and background both qualify.
        let notice_target = request
            .parent_session_id
            .clone()
            .filter(|_| self.notices.is_bound());
        if questionnaire_target.is_none() {
            capabilities.remove(&ToolCapability::Questionnaire);
        }
        let bound = AgentBinder::bind(
            request.definition,
            AgentInvocation {
                role: AgentRole::Delegated,
                available_tools: capabilities,
            },
            &config,
        )?;

        self.spawn_bound(BoundLaunchRequest {
            bound,
            prompt: request.prompt,
            run_id: request.run_id,
            parent_session_id: request.parent_session_id,
            output_file: request.output_file,
            questionnaire_target,
            notice_target,
            frozen_cli: None,
            hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        })
    }

    /// Starts a workflow node from persisted launch metadata without looking up
    /// or rebinding an agent definition.
    pub(crate) fn spawn_frozen(
        &self,
        request: FrozenAgentLaunchRequest,
    ) -> anyhow::Result<AgentRunHandle> {
        let config = self.config.read().expect("delegated config lock").clone();
        let mut current_tools = AgentCapabilities::all_host_tools();
        #[cfg(windows)]
        current_tools.remove(&ToolCapability::Bash);
        #[cfg(not(windows))]
        current_tools.remove(&ToolCapability::Powershell);
        current_tools.remove(&ToolCapability::Questionnaire);
        let bound = AgentBinder::bind_frozen(&request.agent, &config, &current_tools)?;
        let frozen_cli = match request.agent.runtime {
            crate::workflow::AgentRuntime::Rho => None,
            crate::workflow::AgentRuntime::ClaudeCli | crate::workflow::AgentRuntime::Cursor => {
                Some(frozen_cli_launch(request.agent)?)
            }
        };
        self.spawn_bound(BoundLaunchRequest {
            bound,
            prompt: request.prompt,
            run_id: request.run_id,
            parent_session_id: None,
            output_file: request.output_file,
            questionnaire_target: None,
            notice_target: None,
            frozen_cli,
            hook_host_labels: request.hook_host_labels,
        })
    }

    fn spawn_bound(&self, request: BoundLaunchRequest) -> anyhow::Result<AgentRunHandle> {
        let BoundLaunchRequest {
            bound,
            prompt,
            run_id,
            parent_session_id,
            output_file,
            questionnaire_target,
            notice_target,
            frozen_cli,
            hook_host_labels,
        } = request;

        let capacity_class = bound.runtime().capacity_class();
        let RuntimeMessagingPorts {
            messaging,
            steering_slot,
            claude_parent_rx,
        } = MessagingSupport::for_runtime(bound.runtime());

        let mut initial = bound.artifact_identity().starting_status();
        initial.parent_session_id = parent_session_id.as_ref().map(ToString::to_string);
        // Executor owns the Starting boundary; sinks continue_from it.
        // Write Starting here so the handle can observe status before the task runs.
        subagent::initialize_status(&output_file, &initial)?;
        let (status_tx, status) = tokio::sync::watch::channel(initial);
        let (completion_tx, completion) = tokio::sync::watch::channel(false);
        let cancellation = RunCancellation::new();
        let task_cancellation = cancellation.clone();
        let live_title = std::sync::Arc::new(std::sync::Mutex::new(None));
        let title_config = self.config.read().expect("delegated config lock").clone();
        let config_path = self.config_path.clone();
        let cwd = self.cwd.clone();
        let host_input = self.host_input.clone();
        let notices = self.notices.clone();
        let persisted_output = output_file.clone();
        let concurrency = self.concurrency.clone();
        // Auto resolves the classifier template (or builds a fresh one) inside
        // automation; non-Auto keeps approval_session. Both may be set; Auto
        // ignores the session.
        let approval_session = self.approval_session.clone();
        let approval_classifier = self.approval_classifier.clone();
        let task_steering_slot = steering_slot.clone();

        let task_status_tx = status_tx.clone();
        let title_run_id = run_id.clone();
        let title_output = output_file.clone();
        let title_cwd = self.cwd.clone();
        let title_prompt = prompt.clone();
        let title_agent_id = bound.id().to_string();
        let title_slot = std::sync::Arc::clone(&live_title);
        let task_live_title = std::sync::Arc::clone(&live_title);
        let task: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            // Acquire runtime-aware capacity before work starts. Claude runs
            // take Claude capacity first so queued Claude work cannot occupy
            // global permits while waiting on the nested pool; Rho takes only
            // the global pool. Dropping the guard returns every held permit.
            let Some(_runtime_permits) = concurrency
                .acquire(capacity_class, &task_cancellation)
                .await
            else {
                let mut stopped = task_status_tx.borrow().clone();
                stopped.state = RunState::Stopped;
                stopped.last_activity = Some("cancelled before execution".into());
                stopped.mark_finished_now();
                task_status_tx.send_replace(stopped.clone());
                subagent::write_status(&output_file, &stopped)?;
                return Ok(());
            };

            spawn_run_title(
                &title_config,
                title_prompt,
                title_agent_id,
                title_run_id,
                title_cwd,
                RunTitleSinks {
                    output_file: title_output,
                    status_tx: task_status_tx.clone(),
                    live_title: title_slot,
                },
            );

            let started_status = task_status_tx.borrow().clone();
            match into_launch(
                bound,
                prompt.clone(),
                output_file.clone(),
                cwd.clone(),
                task_cancellation.clone(),
                Some(task_status_tx.clone()),
                Some(started_status),
            ) {
                Launch::ClaudeCli(mut session) => {
                    append_child_communication_contract(&mut session.system_prompt);
                    session.parent_messages = claude_parent_rx;
                    session.overrides.live_title = Some(std::sync::Arc::clone(&task_live_title));
                    if let Some(frozen) = frozen_cli {
                        apply_frozen(&mut session.overrides, frozen);
                    }
                    crate::claude_runtime::session::run_session(session).await
                }
                Launch::Cursor(mut session) => {
                    append_child_communication_contract(&mut session.system_prompt);
                    session.overrides.live_title = Some(std::sync::Arc::clone(&task_live_title));
                    if let Some(frozen) = frozen_cli {
                        apply_frozen(&mut session.overrides, frozen);
                    }
                    crate::cursor_runtime::session::run_session(session).await
                }
                Launch::Rho(bound) => {
                    let bound_config = bound
                        .rho_config()
                        .expect("Rho launch has bound config")
                        .clone();
                    run_rho_agent(RhoAgentRun {
                        bound,
                        config: bound_config,
                        config_path,
                        cwd,
                        prompt,
                        output_file,
                        run_id,
                        parent_session_id,
                        questionnaire_target,
                        notice_target,
                        host_input,
                        notices,
                        steering_slot: task_steering_slot,
                        cancellation: task_cancellation,
                        status_tx: task_status_tx,
                        live_title: task_live_title,
                        hook_host_labels,
                        approval_session,
                        approval_classifier,
                    })
                    .await
                }
            }
        });

        let failure_status = status.clone();
        tokio::spawn(async move {
            let failure = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("delegated agent failed: {error:#}")),
                Err(error) if error.is_panic() => Some("delegated agent task panicked".into()),
                Err(error) => Some(format!("delegated agent task failed to join: {error}")),
            };
            if let Some(error) = failure {
                let mut failed = failure_status.borrow().clone();
                if !failed.state.is_terminal() {
                    failed.state = RunState::Error;
                    failed.error = Some(error);
                    failed.mark_finished_now();
                    status_tx.send_replace(failed.clone());
                    let _ = subagent::write_status(&persisted_output, &failed);
                }
            }
            // Close the live window so late parent messages fail closed.
            if let Some(slot) = steering_slot {
                slot.clear();
            }
            completion_tx.send_replace(true);
        });

        Ok(AgentRunHandle {
            cancellation,
            status,
            completion,
            messaging,
        })
    }
}

struct BoundLaunchRequest {
    bound: super::agent_binding::BoundAgent,
    prompt: String,
    run_id: String,
    parent_session_id: Option<rho_sdk::SessionId>,
    output_file: PathBuf,
    /// Parent session that can present this child's questionnaires, when one
    /// is listening. `None` removes the capability before bind.
    questionnaire_target: Option<rho_sdk::SessionId>,
    /// Parent session that can receive this child's notices, when one is
    /// listening. `None` withholds both child notice tools.
    notice_target: Option<rho_sdk::SessionId>,
    frozen_cli: Option<FrozenCliLaunch>,
    hook_host_labels: rho_sdk::hooks::HookHostLabels,
}

struct FrozenCliLaunch {
    executable: PathBuf,
    arguments: Vec<String>,
    executable_identity: crate::workflow::ExecutableIdentity,
    // Keep descriptor-backed paths alive until the child has exited.
    _verified_executable: crate::workflow::VerifiedExecutable,
}

fn frozen_cli_launch(agent: crate::workflow::ResolvedAgent) -> anyhow::Result<FrozenCliLaunch> {
    let identity = agent
        .executable_identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("frozen CLI launch has no executable identity"))?;
    let verified_executable = crate::workflow::verify_executable_identity(identity)?;
    let script_path = crate::workflow::verified_handle_path(
        &verified_executable.executable.file,
        std::path::Path::new(&identity.file.canonical_path),
    )?;
    let mut arguments = agent.arguments;
    let executable = if let Some(interpreter) = &verified_executable.interpreter {
        let interpreter_path = crate::workflow::verified_handle_path(
            &interpreter.file,
            std::path::Path::new(&interpreter.identity.canonical_path),
        )?;
        let mut interpreter_arguments = verified_executable.interpreter_arguments.clone();
        interpreter_arguments.push(crate::paths::display(&script_path));
        interpreter_arguments.extend(arguments);
        arguments = interpreter_arguments;
        interpreter_path
    } else {
        script_path
    };
    Ok(FrozenCliLaunch {
        executable,
        arguments,
        executable_identity: identity.clone(),
        _verified_executable: verified_executable,
    })
}

fn into_launch(
    bound: super::agent_binding::BoundAgent,
    prompt: String,
    output_file: PathBuf,
    cwd: PathBuf,
    cancellation: RunCancellation,
    status_tx: Option<tokio::sync::watch::Sender<RunStatus>>,
    started_status: Option<RunStatus>,
) -> Launch {
    match bound.runtime() {
        super::agent_binding::BoundRuntime::ClaudeCli { .. } => Launch::ClaudeCli(
            bound
                .into_claude_session(
                    prompt,
                    output_file,
                    cwd,
                    cancellation,
                    status_tx,
                    started_status,
                )
                .expect("Claude bound runtime builds a Claude session"),
        ),
        super::agent_binding::BoundRuntime::Cursor { .. } => Launch::Cursor(
            bound
                .into_cursor_session(
                    prompt,
                    output_file,
                    cwd,
                    cancellation,
                    status_tx,
                    started_status,
                )
                .expect("Cursor bound runtime builds a Cursor session"),
        ),
        super::agent_binding::BoundRuntime::Rho { .. } => Launch::Rho(bound),
    }
}

fn apply_frozen(overrides: &mut crate::cli_runtime::CliSessionOverrides, frozen: FrozenCliLaunch) {
    let expected_identity = frozen.executable_identity;
    let verified_executable = frozen._verified_executable;
    overrides.executable = Some(crate::cli_runtime::CliExecutable::from_path(
        frozen.executable,
    ));
    overrides.frozen_argv = Some(frozen.arguments);
    overrides.before_spawn = Some(Box::new(move |command| {
        crate::workflow::verify_executable_identity(&expected_identity)
            .map_err(std::io::Error::other)?;
        let mut files = vec![&verified_executable.executable.file];
        if let Some(interpreter) = &verified_executable.interpreter {
            files.push(&interpreter.file);
        }
        crate::workflow::configure_handle_inheritance(command, &files)
            .map_err(std::io::Error::other)
    }));
}

/// External runtimes receive the same child contract as delegated Rho sessions.
fn append_child_communication_contract(prompt: &mut crate::agent::PromptPolicy) {
    let (crate::agent::PromptPolicy::Extend(text) | crate::agent::PromptPolicy::Replace(text)) =
        prompt;
    text.push_str("\n\n");
    text.push_str(super::subagent_messaging::CHILD_COMMUNICATION_CONTRACT);
}

/// One delegated run on the Rho runtime, after binding and permit acquisition.
struct RhoAgentRun {
    bound: super::agent_binding::BoundAgent,
    config: Config,
    config_path: PathBuf,
    cwd: PathBuf,
    prompt: String,
    output_file: PathBuf,
    run_id: String,
    parent_session_id: Option<rho_sdk::SessionId>,
    questionnaire_target: Option<rho_sdk::SessionId>,
    notice_target: Option<rho_sdk::SessionId>,
    host_input: SubagentHostInputBridge,
    notices: SubagentNoticeBridge,
    steering_slot: Option<SteeringSlot>,
    cancellation: RunCancellation,
    status_tx: tokio::sync::watch::Sender<RunStatus>,
    live_title: crate::run_artifacts::LiveRunTitle,
    hook_host_labels: rho_sdk::hooks::HookHostLabels,
    approval_session: Option<rho_sdk::ApprovalSession>,
    approval_classifier: Option<Arc<ClassifierApprovalHandler>>,
}

/// Drive a delegated run through Rho's own automation loop.
async fn run_rho_agent(run: RhoAgentRun) -> anyhow::Result<()> {
    let RhoAgentRun {
        bound,
        mut config,
        config_path,
        cwd,
        prompt,
        output_file,
        run_id,
        parent_session_id,
        questionnaire_target,
        notice_target,
        host_input,
        notices,
        steering_slot,
        cancellation,
        status_tx,
        live_title,
        hook_host_labels,
        approval_session,
        approval_classifier,
    } = run;

    super::cli_config::prepare_model_metadata(
        &config,
        &crate::credential_store::AppCredentialStore,
        &super::cli_config::ProviderRefreshStatus::NotAttempted,
    )
    .await;
    super::cli_config::normalize_reasoning(&mut config);
    let diagnostics = RuntimeDiagnostics::new(&config);
    diagnostics.update_agent(bound.id().as_str(), &bound.fingerprint().to_string());
    let started_status = status_tx.borrow().clone();
    let mut reporter = RunReporter::continue_from(
        output_file,
        started_status,
        cwd.clone(),
        &prompt,
        /* stream_output */ false,
        Some(status_tx),
        Some(live_title),
    )?;
    let agent_id = bound.id().to_string();
    let max_steps = std::num::NonZeroUsize::new(
        bound
            .step_limit()
            .try_into()
            .map_err(|_| anyhow::anyhow!("agent step limit does not fit this platform"))?,
    )
    .ok_or_else(|| anyhow::anyhow!("agent step limit must be positive"))?;
    let notice_poster = notice_target.map(|parent_session_id| {
        Arc::new(DelegatedNoticePoster {
            run_id: run_id.clone(),
            agent_id: agent_id.clone(),
            parent_session_id,
            notices,
        }) as Arc<dyn NoticePoster>
    });
    let startup = automation::Startup {
        config: &config,
        config_path,
        cwd,
        no_system_prompt: false,
        system_prompt_suffix: Some(super::subagent_messaging::CHILD_COMMUNICATION_CONTRACT),
        no_tools: false,
        no_subagents: true,
        usage_purpose: "subagent",
        parent_session_id: parent_session_id.clone(),
        agent: bound,
        output_file: None,
        output: OutputFormat::Text,
        max_steps: Some(max_steps),
        timeout: None,
        diagnostics,
        herdr: HerdrReporter::default(),
        host_input: questionnaire_target.map(|parent_session_id| {
            Arc::new(SubagentHostInputResponder::new(
                run_id,
                agent_id,
                parent_session_id,
                host_input,
            )) as Arc<dyn super::headless_run::HostInputResponder>
        }),
        notice_poster,
        steering_slot,
        approval_session,
        approval_classifier,
        hook_host_labels,
    };
    let result =
        automation::run_session(prompt, &startup, Some(&mut reporter), Some(cancellation)).await;
    reporter.finish(&result);
    result.map(|_| ())
}

struct DelegatedNoticePoster {
    run_id: String,
    agent_id: String,
    parent_session_id: rho_sdk::SessionId,
    notices: SubagentNoticeBridge,
}

impl NoticePoster for DelegatedNoticePoster {
    fn post(
        &self,
        message: ValidatedMessage,
        delivery: super::subagent_messaging::NoticeDelivery,
    ) -> Result<(), NoticePostError> {
        self.notices.post(SubagentNotice {
            run_id: self.run_id.clone(),
            agent_id: self.agent_id.clone(),
            parent_session_id: self.parent_session_id.clone(),
            message: message.into_string(),
            delivery,
        })
    }
}

/// Whether a delegated Rho run may expose the questionnaire tool.
///
/// Needs a parent session id and a bound parent host-input bridge. Foreground
/// and background launches both qualify; headless or parentless launches do not.
fn delegated_questionnaire_available(
    parent_session_id: Option<&rho_sdk::SessionId>,
    host_input_bound: bool,
) -> bool {
    parent_session_id.is_some() && host_input_bound
}

struct RunTitleSinks {
    output_file: PathBuf,
    status_tx: tokio::sync::watch::Sender<RunStatus>,
    live_title: crate::run_artifacts::LiveRunTitle,
}

fn spawn_run_title(
    config: &Config,
    prompt: String,
    agent_id: String,
    run_id: String,
    workspace_path: PathBuf,
    sinks: RunTitleSinks,
) {
    let model = crate::title::title_model_from_config(config);
    let session_id =
        rho_sdk::SessionId::from_string(run_id).unwrap_or_else(|_| rho_sdk::SessionId::new());
    tokio::spawn(async move {
        let usage_recording = crate::usage::default_recording().await;
        let cancellation = rho_sdk::CancellationToken::new();
        let title = crate::title::generate_title(
            model,
            format!("Role: {agent_id}\n\nDelegated agent run:\n{prompt}"),
            session_id,
            workspace_path,
            usage_recording,
            cancellation,
        )
        .await;
        let Ok(title) = title else {
            return;
        };
        {
            let mut slot = sinks
                .live_title
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot.is_none() {
                *slot = Some(title.clone());
            }
        }
        sinks.status_tx.send_modify(|status| {
            if status.title.is_none() {
                status.title = Some(title.clone());
            }
        });
        let _ = subagent::apply_generated_title(&sinks.output_file, &title);
    });
}

#[cfg(test)]
#[path = "agent_executor_tests.rs"]
mod tests;
