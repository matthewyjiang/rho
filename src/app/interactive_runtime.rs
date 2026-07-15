use std::{num::NonZeroU64, path::PathBuf, sync::Arc};

use rho_sdk::{
    model::{ContentBlock, Message, ModelRequest, ModelResponse},
    provider::ModelProvider,
    CapabilityRequest, CompactionFuture, CompactionOutput, CompactionRequest, Compactor, Error,
    HostInputId, HostInputResponse, PolicyDecision, Rho, Run, RunEvent, RunOutcome, Session,
    SessionId, SessionOptions, SystemPrompt, UserInput, Workspace, WorkspacePolicy,
};

use crate::{
    compaction::{
        build_summary_request_messages, partition_messages_for_compaction,
        replacement_history_from_summary, CompactionConfig,
    },
    config::Config,
    credentials::OsCredentialStore,
    diagnostics::RuntimeDiagnostics,
    model::models_dev::cached_model_metadata,
    prompt,
    providers::{build_sdk_provider_with_source, UnavailableProvider},
    session::Session as StoredSession,
    tools::sdk_registry::{AppToolSet, ToolSetOptions},
};

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
    pub(crate) cwd: PathBuf,
    pub(crate) no_system_prompt: bool,
    pub(crate) no_tools: bool,
    pub(crate) questionnaire_enabled: bool,
    pub(crate) history: Vec<Message>,
    pub(crate) session_id: Option<String>,
    pub(crate) storage: Option<StoredSession>,
    pub(crate) diagnostics: RuntimeDiagnostics,
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
            cwd,
            no_system_prompt,
            no_tools,
            questionnaire_enabled,
            history,
            session_id,
            storage,
            diagnostics,
            unavailable_error,
        } = options;
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
        let tools = if no_tools {
            AppToolSet::disabled()
        } else {
            AppToolSet::new(
                config,
                diagnostics.clone(),
                ToolSetOptions::new().questionnaire(questionnaire_enabled),
            )
        };
        let specs = tools.specs();
        let system_prompt = if no_system_prompt {
            diagnostics.update_prompt_sources(Vec::new());
            SystemPrompt::None
        } else {
            let built = prompt::system_prompt(&specs, &cwd);
            diagnostics.update_prompt_sources(built.sources);
            SystemPrompt::Custom(built.text)
        };
        diagnostics.update_tools(&specs);
        let workspace = Workspace::new(&sdk_options.workspace.root)?;
        let context_window = cached_model_metadata(&config.provider, &config.model)
            .and_then(|metadata| metadata.display_context_window());
        let compaction = sdk_options.runtime.compaction.clone();
        diagnostics.update_compaction_config(&compaction);
        let runtime = build_runtime(
            Arc::clone(&provider),
            &tools,
            workspace.clone(),
            system_prompt.clone(),
            sdk_options.runtime.reasoning,
            compaction.clone(),
            context_window,
        )?;
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
            let mut options = SessionOptions::new().history(history);
            if let Some(id) = session_id.as_deref() {
                options = options
                    .id(SessionId::from_string(id)?)
                    .prompt_cache_key(cache_key.unwrap_or_else(|| prompt_cache_key(id)));
            }
            options
        };
        let session = runtime.session(options).await?;
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
    }

    pub(crate) fn take_context_usage(&mut self) -> Option<rho_sdk::model::ContextUsage> {
        self.pending_context_usage.take()
    }

    /// Warnings queued while the TUI owns the terminal (e.g. resume omissions).
    pub(crate) fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_notices)
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

    pub(crate) async fn steer(&mut self, input: UserInput) -> Result<(), Error> {
        self.active_run
            .as_ref()
            .ok_or(Error::InvalidHostResponse {
                message: "no active run accepts steering input".into(),
            })?
            .steer(input)
            .await?;
        self.state = InteractiveState::Running(RunPhase::Steering);
        Ok(())
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
        self.pending_session_id = Some(SessionId::new());
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
        self.state = InteractiveState::Idle;
        Ok(report)
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

    fn observe_event(&mut self, event: &RunEvent) {
        self.state = state_after_event(self.state, event);
        match event {
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
        let replacement_runtime = build_runtime(
            Arc::clone(&self.provider),
            &self.tools,
            self.workspace.clone(),
            self.system_prompt.clone(),
            self.reasoning,
            self.compaction.clone(),
            self.context_window,
        )?;
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

fn build_runtime(
    provider: Arc<dyn ModelProvider>,
    tools: &AppToolSet,
    workspace: Workspace,
    system_prompt: SystemPrompt,
    reasoning: rho_sdk::ReasoningLevel,
    compaction: CompactionConfig,
    context_window: Option<u64>,
) -> Result<Rho, Error> {
    let automatic_compaction_threshold = context_window
        .and_then(|window| compaction.threshold_tokens(window))
        .and_then(NonZeroU64::new);
    let compactor = ModelCompactor {
        provider: Arc::clone(&provider),
        tool_specs: tools.specs(),
        reasoning,
        config: compaction,
        context_window,
    };
    let mut builder = Rho::builder()
        .provider_shared(provider)
        .system_prompt(system_prompt)
        .workspace(workspace)
        .workspace_policy(InteractiveWorkspacePolicy)
        .reasoning_level(reasoning)
        .max_steps(super::sdk_config::run_step_limit())
        .compactor(compactor);
    if let Some(trigger_tokens) = automatic_compaction_threshold {
        builder =
            builder.compaction_policy(rho_sdk::CompactionPolicy::at_context_tokens(trigger_tokens));
    }
    for tool in tools.tools() {
        builder = builder.tool_shared(tool.clone());
    }
    builder.build()
}

struct InteractiveWorkspacePolicy;

impl WorkspacePolicy for InteractiveWorkspacePolicy {
    fn evaluate(&self, _request: &CapabilityRequest) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

struct ModelCompactor {
    provider: Arc<dyn ModelProvider>,
    tool_specs: Vec<rho_sdk::model::ToolSpec>,
    reasoning: rho_sdk::ReasoningLevel,
    config: CompactionConfig,
    context_window: Option<u64>,
}

impl Compactor for ModelCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(async move {
            let messages = request.messages().to_vec();
            let target_tokens = self
                .context_window
                .map(|window| self.config.target_tokens(window))
                .unwrap_or(u64::MAX / 2);
            let Some(partition) =
                partition_messages_for_compaction(&messages, &self.tool_specs, target_tokens)
            else {
                return CompactionOutput::new(messages);
            };
            let summary_messages = build_summary_request_messages(&partition.compacted_messages);
            let model_request = ModelRequest {
                messages: &summary_messages,
                tools: &[],
                cancellation: request.cancellation().clone(),
                reasoning_level: self.reasoning,
                prompt_cache_key: None,
            };
            let response = self.provider.send_turn(model_request).await?;
            let ModelResponse::Assistant(blocks) = response;
            let summary = blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            if summary.trim().is_empty() {
                return Err(Error::InvalidHostResponse {
                    message: "compaction model returned no summary text".into(),
                });
            }
            CompactionOutput::new(replacement_history_from_summary(partition, summary))
        })
    }
}

#[cfg(test)]
#[path = "interactive_runtime_tests.rs"]
mod tests;
