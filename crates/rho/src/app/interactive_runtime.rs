use std::{future::Future, num::NonZeroUsize, path::PathBuf, pin::Pin, sync::Arc};

use rho_sdk::{
    model::Message, provider::ModelProvider, ApprovalHandler, ApprovalRequestReceiver, Error,
    HostInputId, HostInputResponse, Rho, Run, RunEvent, RunOutcome, Session, SessionId,
    SessionOptions, SystemPrompt, UserInput, Workspace,
};

use crate::{
    agent::PromptPolicy,
    compaction::CompactionConfig,
    config::Config,
    credentials::OsCredentialStore,
    diagnostics::RuntimeDiagnostics,
    permission::PermissionMode,
    prompt,
    providers::{build_sdk_provider_with_source, UnavailableProvider},
    session::Session as StoredSession,
    tools::sdk_registry::{AppToolSet, ToolSetOptions},
};

use super::{
    agent_binding::BoundAgent,
    policy::AppPolicy,
    runtime_builder::{
        build_compaction, build_runtime, configured_context_window, RuntimeBuildOptions,
    },
};

pub(crate) type SteeringAcceptanceFuture =
    Pin<Box<dyn Future<Output = Result<rho_sdk::SteeringId, Error>> + Send>>;
pub(crate) type SteeringRetractionFuture =
    Pin<Box<dyn Future<Output = Result<rho_sdk::SteeringRetraction, Error>> + Send>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum InteractiveState {
    #[default]
    Idle,
    Running(RunPhase),
    WaitingForHostInput,
    Cancelling(RunPhase),
    Compacting,
    SwitchingProvider,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunPhase {
    Model,
    Tool,
    Steering,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveRunCommand {
    Quit,
    SwitchSession,
    ReplaceProvider,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveRunDisposition {
    CancelAndWait,
    RejectUntilFinished,
    DeferUntilFinished,
}

pub(crate) const fn active_run_disposition(command: ActiveRunCommand) -> ActiveRunDisposition {
    match command {
        ActiveRunCommand::Quit => ActiveRunDisposition::CancelAndWait,
        ActiveRunCommand::SwitchSession => ActiveRunDisposition::RejectUntilFinished,
        ActiveRunCommand::ReplaceProvider => ActiveRunDisposition::DeferUntilFinished,
    }
}

pub(crate) struct InteractiveRuntimeOptions<'a> {
    pub(crate) config: &'a Config,
    pub(crate) config_path: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) no_system_prompt: bool,
    pub(crate) no_tools: bool,
    pub(crate) no_subagents: bool,
    pub(crate) questionnaire_enabled: bool,
    pub(crate) history: Vec<Message>,
    pub(crate) session_id: Option<String>,
    pub(crate) storage: Option<StoredSession>,
    pub(crate) diagnostics: RuntimeDiagnostics,
    pub(crate) agent: BoundAgent,
    pub(crate) unavailable_error: Option<crate::model::ModelError>,
}

enum ReplacementSessionSource<'a> {
    History {
        history: Vec<Message>,
        id: Option<String>,
    },
    Snapshot {
        storage: &'a StoredSession,
        id: String,
    },
}

pub(crate) struct InteractiveRuntime {
    runtime: Rho,
    session: Session,
    active_run: Option<Run>,
    state: InteractiveState,
    provider: Arc<dyn ModelProvider>,
    tools: AppToolSet,
    workspace: Workspace,
    system_prompt: SystemPrompt,
    reasoning: rho_sdk::ReasoningLevel,
    compaction: CompactionConfig,
    context_window: Option<u64>,
    permission_mode: PermissionMode,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    approval_receiver: Option<ApprovalRequestReceiver>,
    agent_id: String,
    agent_fingerprint: String,
    storage: Option<StoredSession>,
    pending_model_user: Option<Message>,
    pending_display_user: Option<Message>,
    pending_history_start: Option<usize>,
    pending_session_id: Option<SessionId>,
    pending_context_usage: Option<rho_sdk::model::ContextUsage>,
    pending_notices: Vec<String>,
    cumulative_input_tokens: u64,
    step_input_token_baseline: u64,
}

impl InteractiveRuntime {
    pub(crate) async fn new(options: InteractiveRuntimeOptions<'_>) -> anyhow::Result<Self> {
        let InteractiveRuntimeOptions {
            config,
            config_path,
            cwd,
            no_system_prompt,
            no_tools,
            no_subagents,
            questionnaire_enabled,
            history,
            session_id,
            storage,
            diagnostics,
            agent,
            unavailable_error,
        } = options;
        let agent_id = agent.id().to_string();
        let agent_fingerprint = agent.fingerprint().to_string();
        let sdk_options = super::sdk_config::SdkBootstrapOptions::from_config(config, &cwd)?;
        let provider: Arc<dyn ModelProvider> = match unavailable_error {
            Some(error) => Arc::new(UnavailableProvider::new(error)),
            None => {
                let credentials =
                    crate::auth::provider_credentials::ApplicationCredentialSource::new(Arc::new(
                        OsCredentialStore,
                    ));
                build_sdk_provider_with_source(sdk_options.provider.clone(), &credentials)?
            }
        };
        let delegation_available = config.enable_subagents && !no_subagents;
        let launch_delegation_enabled = delegation_available && agent.tools().contains("agent");
        let delegation_enabled =
            launch_delegation_enabled || (delegation_available && agent.tools().contains("agents"));
        let mut tools = if no_tools {
            AppToolSet::disabled()
        } else {
            let delegation_cwd = delegation_enabled.then(|| cwd.clone());
            AppToolSet::new(
                config,
                diagnostics.clone(),
                ToolSetOptions::new()
                    .questionnaire(questionnaire_enabled)
                    .delegation_tools(delegation_cwd, agent.tools())
                    .subagent_config_path(config_path)
                    .background_subagents(true),
            )
        };
        let allowed = agent.tools().iter().cloned().collect::<Vec<_>>();
        tools.retain_named(&allowed);
        let specs = tools.specs();
        let system_prompt = if no_system_prompt {
            diagnostics.update_prompt_sources(Vec::new());
            SystemPrompt::None
        } else {
            let mut text = match agent.prompt() {
                PromptPolicy::Replace(text) => text.clone(),
                PromptPolicy::Extend(extra) => {
                    let mut built = prompt::system_prompt(&specs, &cwd);
                    diagnostics.update_prompt_sources(built.sources);
                    if !launch_delegation_enabled {
                        prompt::append_subagents_disabled_instruction(&mut built.text);
                    }
                    if !extra.is_empty() {
                        built.text.push_str("\n\n# Agent instructions\n\n");
                        built.text.push_str(extra);
                    }
                    built.text
                }
            };
            if text.is_empty() {
                text = "You are a coding agent.".into();
            }
            SystemPrompt::Custom(text)
        };
        diagnostics.update_tools(&specs);
        let workspace = Workspace::new(&sdk_options.workspace.root)?;
        let context_window = configured_context_window(config);
        let compaction = sdk_options.runtime.compaction.clone();
        let permission_mode = config.permission_mode;
        let (approval_handler, approval_receiver) = approval_channel_for(permission_mode);
        diagnostics.update_compaction_config(&compaction);
        let runtime = build_runtime(RuntimeBuildOptions {
            provider: Arc::clone(&provider),
            tools: tools.tools(),
            workspace: workspace.clone(),
            workspace_policy: AppPolicy::for_mode(permission_mode),
            approval_handler: approval_handler.clone(),
            system_prompt: system_prompt.clone(),
            reasoning: sdk_options.runtime.reasoning,
            compaction: compaction.clone(),
            context_window,
        })?;
        let cache_key = session_id.as_deref().map(prompt_cache_key);
        let resumed_snapshot = storage
            .as_ref()
            .map(|storage| {
                storage.snapshot_for_resume(
                    provider.identity(),
                    cache_key
                        .clone()
                        .unwrap_or_else(|| prompt_cache_key(storage.id())),
                )
            })
            .transpose()?;
        let options = if let Some(snapshot) = resumed_snapshot {
            // The TUI has not started yet, so stderr is still safe here.
            if let Some(notice) = resume_omissions_notice(&snapshot, &provider.identity()) {
                eprintln!("warning: {notice}");
            }
            SessionOptions::from_snapshot(snapshot)
        } else {
            // Always seed a prompt-cache key, including brand-new sessions that
            // do not yet have durable storage. ensure_session later reuses this
            // session id when creating the on-disk transcript.
            let id = match session_id.as_deref() {
                Some(id) => SessionId::from_string(id)?,
                None => SessionId::new(),
            };
            SessionOptions::new()
                .history(history)
                .id(id.clone())
                .prompt_cache_key(prompt_cache_key(id.as_str()))
        };
        let session = runtime.session(options).await?;
        if let Some(manager) = tools.subagents() {
            manager.set_session(session.id().to_string());
        }
        Ok(Self {
            runtime,
            session,
            active_run: None,
            state: InteractiveState::Idle,
            provider,
            tools,
            workspace,
            system_prompt,
            reasoning: sdk_options.runtime.reasoning,
            compaction,
            context_window,
            permission_mode,
            approval_handler,
            approval_receiver,
            agent_id,
            agent_fingerprint,
            storage,
            pending_model_user: None,
            pending_display_user: None,
            pending_history_start: None,
            pending_session_id: None,
            pending_context_usage: None,
            pending_notices: Vec::new(),
            cumulative_input_tokens: 0,
            step_input_token_baseline: 0,
        })
    }

    pub(crate) fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }

    /// Rebuilds the SDK runtime so the requested permission mode applies to the next turn.
    pub(crate) async fn set_permission_mode(&mut self, mode: PermissionMode) -> anyhow::Result<()> {
        if self.active_run.is_some() {
            anyhow::bail!("permission mode cannot change while a run is active");
        }
        if self.permission_mode == mode {
            return Ok(());
        }

        let snapshot = self.session.snapshot();
        let (approval_handler, approval_receiver) = approval_channel_for(mode);
        let replacement_runtime = build_runtime(RuntimeBuildOptions {
            provider: Arc::clone(&self.provider),
            tools: self.tools.tools(),
            workspace: self.workspace.clone(),
            workspace_policy: AppPolicy::for_mode(mode),
            approval_handler: approval_handler.clone(),
            system_prompt: self.system_prompt.clone(),
            reasoning: self.reasoning,
            compaction: self.compaction.clone(),
            context_window: self.context_window,
        })?;
        let replacement_session = replacement_runtime
            .session(SessionOptions::from_snapshot(snapshot))
            .await?;

        let previous_runtime = std::mem::replace(&mut self.runtime, replacement_runtime);
        self.session = replacement_session;
        self.permission_mode = mode;
        self.approval_handler = approval_handler;
        self.approval_receiver = approval_receiver;
        if let Some(manager) = self.tools.subagents() {
            manager.update_permission_mode(mode);
        }
        previous_runtime.shutdown();
        Ok(())
    }

    pub(crate) fn approval_receiver(&mut self) -> Option<&mut ApprovalRequestReceiver> {
        self.approval_receiver.as_mut()
    }

    pub(crate) fn history(&self) -> Vec<Message> {
        self.session.history()
    }

    pub(crate) fn session_id(&self) -> &SessionId {
        self.pending_session_id
            .as_ref()
            .unwrap_or_else(|| self.session.id())
    }

    pub(crate) fn set_context_window(&mut self, context_window: Option<u64>) {
        self.context_window = context_window;
        if self.active_run.is_none() {
            let _ = self.refresh_compaction();
        }
    }

    pub(crate) fn take_context_usage(&mut self) -> Option<rho_sdk::model::ContextUsage> {
        self.pending_context_usage.take()
    }

    /// Warnings queued while the TUI owns the terminal (e.g. resume omissions).
    pub(crate) fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_notices)
    }

    pub(crate) fn agent_identity(&self) -> (&str, &str) {
        (&self.agent_id, &self.agent_fingerprint)
    }

    pub(crate) fn attach_storage(&mut self, storage: StoredSession) {
        self.storage = Some(storage);
    }

    pub(crate) async fn start(
        &mut self,
        input: UserInput,
        display_user: Option<Message>,
    ) -> Result<(), Error> {
        if self.state != InteractiveState::Idle {
            return Err(Error::SessionBusy);
        }
        if let Some(id) = self.pending_session_id.clone() {
            let storage = self.storage.clone();
            let source = storage.as_ref().map_or_else(
                || ReplacementSessionSource::History {
                    history: Vec::new(),
                    id: Some(id.to_string()),
                },
                |storage| ReplacementSessionSource::Snapshot {
                    storage,
                    id: id.to_string(),
                },
            );
            self.rebuild_session(source)
                .await
                .map_err(|error| Error::Persistence {
                    message: error.to_string(),
                })?;
        }
        let model_user = Message::User(input.blocks().to_vec());
        let mut request_history = self.session.history();
        self.pending_history_start = Some(request_history.len());
        self.pending_model_user = Some(model_user);
        self.pending_display_user = display_user;
        self.cumulative_input_tokens = 0;
        self.step_input_token_baseline = 0;
        request_history.push(Message::User(input.blocks().to_vec()));
        self.pending_context_usage = Some(rho_sdk::model::ContextUsage::estimated(
            rho_sdk::model::context::estimate_context_tokens(&request_history, &self.tools.specs()),
            self.context_window,
        ));
        self.active_run = Some(self.session.start(input).await?);
        self.state = InteractiveState::Running(RunPhase::Model);
        Ok(())
    }

    pub(crate) async fn next_event(&mut self) -> Option<RunEvent> {
        let event = self.active_run.as_mut()?.next_event().await;
        if let Some(event) = &event {
            self.observe_event(event);
        }
        event
    }

    pub(crate) fn cancel(&mut self) {
        if let Some(run) = &self.active_run {
            let phase = match self.state {
                InteractiveState::Running(phase) | InteractiveState::Cancelling(phase) => phase,
                InteractiveState::WaitingForHostInput => RunPhase::Tool,
                _ => RunPhase::Model,
            };
            run.cancel();
            self.state = InteractiveState::Cancelling(phase);
        }
    }

    pub(crate) fn request_steer(
        &mut self,
        input: UserInput,
    ) -> Result<SteeringAcceptanceFuture, Error> {
        let receipt = self
            .active_run
            .as_ref()
            .ok_or(Error::InvalidHostResponse {
                message: "no active run accepts steering input".into(),
            })?
            .request_steer_retractable(input)?;
        self.state = InteractiveState::Running(RunPhase::Steering);
        Ok(Box::pin(receipt))
    }

    pub(crate) fn request_steering_retraction(
        &self,
        id: rho_sdk::SteeringId,
    ) -> Result<SteeringRetractionFuture, Error> {
        let receipt = self
            .active_run
            .as_ref()
            .ok_or(Error::InvalidHostResponse {
                message: "no active run accepts steering retractions".into(),
            })?
            .request_steering_retraction(id)?;
        Ok(Box::pin(receipt))
    }

    pub(crate) async fn respond(
        &mut self,
        request_id: HostInputId,
        response: HostInputResponse,
    ) -> Result<(), Error> {
        self.active_run
            .as_ref()
            .ok_or(Error::InvalidHostResponse {
                message: "no active run accepts host input".into(),
            })?
            .respond(request_id, response)
            .await?;
        self.state = InteractiveState::Running(RunPhase::Tool);
        Ok(())
    }

    pub(crate) async fn finish_run(&mut self) -> anyhow::Result<RunOutcome> {
        let mut run = self
            .active_run
            .take()
            .ok_or_else(|| anyhow::anyhow!("no active run"))?;
        let outcome = run.outcome().await;
        let storage_result = self.sync_storage(outcome.as_ref().ok());
        self.pending_model_user = None;
        self.pending_display_user = None;
        self.pending_history_start = None;
        self.state = InteractiveState::Idle;
        storage_result?;
        Ok(outcome?)
    }

    pub(crate) async fn compact(&mut self) -> anyhow::Result<bool> {
        if self.active_run.is_some() {
            anyhow::bail!("session is busy");
        }
        let outcome = self.session.compact().await?;
        self.sync_storage_replace()?;
        if outcome.current_messages() < outcome.previous_messages() {
            self.pending_context_usage = Some(
                rho_sdk::model::ContextUsage::unknown_after_compaction(self.context_window),
            );
        }
        Ok(outcome.current_messages() < outcome.previous_messages())
    }

    pub(crate) fn reset(&mut self) -> anyhow::Result<()> {
        if self.active_run.is_some() {
            anyhow::bail!("cannot reset while a run is active");
        }
        self.session.reset()?;
        self.storage = None;
        let session_id = SessionId::new();
        if let Some(manager) = self.tools.subagents() {
            manager.set_session(session_id.to_string());
        }
        self.pending_session_id = Some(session_id);
        self.state = InteractiveState::Idle;
        Ok(())
    }

    pub(crate) async fn resume(
        &mut self,
        storage: StoredSession,
        _history: Vec<Message>,
    ) -> anyhow::Result<()> {
        if self.active_run.is_some() {
            debug_assert_eq!(
                active_run_disposition(ActiveRunCommand::SwitchSession),
                ActiveRunDisposition::RejectUntilFinished
            );
            anyhow::bail!("cannot switch sessions while a run is active");
        }
        let id = storage.id().to_string();
        self.rebuild_session(ReplacementSessionSource::Snapshot {
            storage: &storage,
            id,
        })
        .await?;
        if let Some(manager) = self.tools.subagents() {
            manager.set_session(self.session.id().to_string());
        }
        self.storage = Some(storage);
        Ok(())
    }

    pub(crate) fn replace_provider(
        &mut self,
        provider: Arc<dyn ModelProvider>,
        reasoning: rho_sdk::ReasoningLevel,
    ) -> Result<rho_sdk::model::handoff::HandoffReport, Error> {
        if self.active_run.is_some() {
            debug_assert_eq!(
                active_run_disposition(ActiveRunCommand::ReplaceProvider),
                ActiveRunDisposition::DeferUntilFinished
            );
            return Err(Error::SessionBusy);
        }
        self.state = begin_provider_switch(self.state)?;
        let report = match self.session.replace_provider(Arc::clone(&provider)) {
            Ok(report) => report,
            Err(error) => {
                self.state = InteractiveState::Idle;
                return Err(error);
            }
        };
        if let Err(error) = self.session.set_reasoning_level(reasoning) {
            self.state = InteractiveState::Idle;
            return Err(error);
        }
        self.provider = provider;
        self.reasoning = reasoning;
        if let Err(error) = self.refresh_compaction() {
            self.state = InteractiveState::Idle;
            return Err(error);
        }
        let identity = self.provider.identity();
        if let Some(manager) = self.tools.subagents() {
            manager.update_model(&identity.provider, &identity.model, reasoning);
        }
        self.state = InteractiveState::Idle;
        Ok(report)
    }

    fn refresh_compaction(&mut self) -> Result<(), Error> {
        let (compactor, policy) = build_compaction(
            Arc::clone(&self.provider),
            self.tools.tools(),
            self.reasoning,
            self.compaction.clone(),
            self.context_window,
        );
        self.session
            .set_compaction(Some(Arc::new(compactor)), policy)
    }

    pub(crate) fn append_user_context_with_display(
        &mut self,
        model: String,
        display: String,
    ) -> anyhow::Result<()> {
        let message = Message::user_text(model);
        self.session.append_message(message.clone())?;
        if let Some(storage) = &self.storage {
            storage.save_snapshot(&self.session.snapshot(), &[Message::user_text(display)])?;
        }
        Ok(())
    }

    pub(crate) fn load_skill(
        &mut self,
        skill: &crate::skills::Skill,
        max_bytes: usize,
    ) -> anyhow::Result<()> {
        let content = crate::tool::truncate(skill.contents.clone(), max_bytes);
        let message = Message::user_text(format!(
            "Loaded skill `{}` from {}:\n\n{}",
            skill.name, skill.source, content
        ));
        self.session.append_message(message.clone())?;
        if let Some(storage) = &self.storage {
            storage.save_snapshot(&self.session.snapshot(), std::slice::from_ref(&message))?;
        }
        Ok(())
    }

    pub(crate) async fn shutdown(&mut self) {
        if self.active_run.is_some() {
            debug_assert_eq!(
                active_run_disposition(ActiveRunCommand::Quit),
                ActiveRunDisposition::CancelAndWait
            );
            self.cancel();
            let _ = self.finish_run().await;
        }
        self.runtime.shutdown();
        self.tools.shutdown().await;
    }

    pub(crate) fn subagents(&self) -> Option<&crate::tools::agent::SubagentManager> {
        self.tools.subagents()
    }

    fn observe_event(&mut self, event: &RunEvent) {
        self.state = state_after_event(self.state, event);
        match event {
            RunEvent::Started { .. } => {
                self.cumulative_input_tokens = 0;
                self.step_input_token_baseline = 0;
            }
            RunEvent::StepStarted { .. } => {
                self.step_input_token_baseline = self.cumulative_input_tokens;
            }
            RunEvent::UsageUpdated { usage } => {
                if let Some(cumulative_tokens) = usage.total_input_tokens() {
                    self.cumulative_input_tokens = cumulative_tokens;
                    let tokens = cumulative_tokens.saturating_sub(self.step_input_token_baseline);
                    let context_window = match (usage.context_window, self.context_window) {
                        (Some(reported), Some(configured)) => Some(reported.min(configured)),
                        (reported, configured) => reported.or(configured),
                    };
                    self.pending_context_usage = Some(
                        rho_sdk::model::ContextUsage::provider_reported(tokens, context_window),
                    );
                }
            }
            RunEvent::CompactionCompleted { .. } => {
                self.pending_context_usage = Some(
                    rho_sdk::model::ContextUsage::unknown_after_compaction(self.context_window),
                );
            }
            _ => {}
        }
    }

    async fn rebuild_session(
        &mut self,
        source: ReplacementSessionSource<'_>,
    ) -> anyhow::Result<()> {
        let (options, resume_notice) = match source {
            ReplacementSessionSource::Snapshot { storage, id } => {
                let snapshot =
                    storage.snapshot_for_resume(self.provider.identity(), prompt_cache_key(&id))?;
                let notice = resume_omissions_notice(&snapshot, &self.provider.identity());
                (SessionOptions::from_snapshot(snapshot), notice)
            }
            ReplacementSessionSource::History { history, id } => {
                let mut options = SessionOptions::new().history(history);
                if let Some(id) = id {
                    options = options
                        .id(SessionId::from_string(&id)?)
                        .prompt_cache_key(prompt_cache_key(&id));
                }
                (options, None)
            }
        };
        let replacement_runtime = build_runtime(RuntimeBuildOptions {
            provider: Arc::clone(&self.provider),
            tools: self.tools.tools(),
            workspace: self.workspace.clone(),
            workspace_policy: AppPolicy::for_mode(self.permission_mode),
            approval_handler: self.approval_handler.clone(),
            system_prompt: self.system_prompt.clone(),
            reasoning: self.reasoning,
            compaction: self.compaction.clone(),
            context_window: self.context_window,
        })?;
        let replacement_session = replacement_runtime.session(options).await?;
        let previous_runtime = std::mem::replace(&mut self.runtime, replacement_runtime);
        self.session = replacement_session;
        if let Some(notice) = resume_notice {
            self.pending_notices.push(notice);
        }
        self.pending_session_id = None;
        self.state = InteractiveState::Idle;
        previous_runtime.shutdown();
        Ok(())
    }

    fn sync_storage(&mut self, outcome: Option<&RunOutcome>) -> anyhow::Result<()> {
        let history = self.session.history();
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        let history_start = self.pending_history_start.unwrap_or(history.len());
        let current_turn_committed = self
            .pending_model_user
            .as_ref()
            .is_some_and(|user| history.get(history_start) == Some(user));
        let mut display_tail = if current_turn_committed {
            history[history_start..].to_vec()
        } else {
            self.pending_model_user
                .clone()
                .into_iter()
                .chain(outcome.and_then(|outcome| {
                    (!outcome.text().is_empty())
                        .then(|| Message::assistant_text(outcome.text().to_string()))
                }))
                .collect()
        };
        if let (Some(display), Some(first)) = (&self.pending_display_user, display_tail.first_mut())
        {
            *first = display.clone();
        }
        storage.save_snapshot(&self.session.snapshot(), &display_tail)?;
        Ok(())
    }

    fn sync_storage_replace(&mut self) -> anyhow::Result<()> {
        if let Some(storage) = &self.storage {
            storage.save_snapshot(&self.session.snapshot(), &[])?;
        }
        Ok(())
    }
}

fn approval_channel_for(
    mode: PermissionMode,
) -> (
    Option<Arc<dyn ApprovalHandler>>,
    Option<ApprovalRequestReceiver>,
) {
    match mode {
        PermissionMode::Supervised => {
            let capacity = NonZeroUsize::new(16).expect("approval channel capacity is non-zero");
            let (handler, receiver) = rho_sdk::approval_channel(capacity);
            (Some(Arc::new(handler)), Some(receiver))
        }
        PermissionMode::Auto | PermissionMode::Plan => (None, None),
    }
}

fn prompt_cache_key(id: &str) -> String {
    crate::providers::openai::prompt_cache_key_from_session_id(id)
        .unwrap_or_else(|| format!("rho:{id}"))
}

fn resume_omissions_notice(
    snapshot: &rho_sdk::SessionSnapshot,
    target: &rho_sdk::model::ModelIdentity,
) -> Option<String> {
    let report = snapshot.provider_context_omissions(target);
    report.has_omissions().then(|| {
        format!(
            "omitted {} incompatible provider-native context block(s) while resuming session (kinds: {})",
            report.omitted_provider_context,
            report.omitted_kinds.join(", ")
        )
    })
}

fn begin_provider_switch(current: InteractiveState) -> Result<InteractiveState, Error> {
    if current == InteractiveState::Idle {
        Ok(InteractiveState::SwitchingProvider)
    } else {
        Err(Error::SessionBusy)
    }
}

fn state_after_event(current: InteractiveState, event: &RunEvent) -> InteractiveState {
    match event {
        RunEvent::Started { .. } | RunEvent::StepStarted { .. } => {
            running_unless_cancelling(current, RunPhase::Model)
        }
        RunEvent::ToolStarted { .. } => running_unless_cancelling(current, RunPhase::Tool),
        RunEvent::ToolFinished { .. } => running_unless_cancelling(current, RunPhase::Model),
        RunEvent::HostInputRequested { .. } => {
            if matches!(current, InteractiveState::Cancelling(_)) {
                current
            } else {
                InteractiveState::WaitingForHostInput
            }
        }
        RunEvent::CompactionStarted { .. } => {
            if matches!(current, InteractiveState::Cancelling(_)) {
                current
            } else {
                InteractiveState::Compacting
            }
        }
        RunEvent::CompactionCompleted { .. } => running_unless_cancelling(current, RunPhase::Model),
        RunEvent::Completed { .. } | RunEvent::Cancelled { .. } => InteractiveState::Completed,
        RunEvent::Failed { .. } => InteractiveState::Failed,
        _ => current,
    }
}

fn running_unless_cancelling(current: InteractiveState, phase: RunPhase) -> InteractiveState {
    if matches!(current, InteractiveState::Cancelling(_)) {
        current
    } else {
        InteractiveState::Running(phase)
    }
}

#[cfg(test)]
#[path = "interactive_runtime_tests.rs"]
mod tests;
