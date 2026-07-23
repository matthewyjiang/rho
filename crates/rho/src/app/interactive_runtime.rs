use std::{num::NonZeroUsize, path::PathBuf, sync::Arc};

use rho_sdk::{
    model::{Message, ToolCall},
    provider::ModelProvider,
    ApprovalHandler, ApprovalRequestReceiver, Error, HostInputId, HostInputResponse, Rho, RunEvent,
    RunOutcome, SessionId, SessionOptions, SystemPrompt, UserInput, Workspace,
};

use {
    crate::agent::{PromptPolicy, ToolCapability},
    crate::compaction::CompactionConfig,
    crate::config::Config,
    crate::credential_store::AppCredentialStore,
    crate::diagnostics::RuntimeDiagnostics,
    crate::permission::PermissionMode,
    crate::prompt,
    crate::session::Session as StoredSession,
    crate::tools::{
        agent::BackgroundSubagents,
        sdk_registry::{AppToolSet, DelegationConfig, ToolSetOptions},
    },
    rho_providers::providers::{build_sdk_provider_with_source, UnavailableProvider},
};

use super::{
    agent_binding::BoundAgent,
    interactive_run_controller::{InteractiveRunController, PendingTurn},
    interactive_session_controller::{InteractiveSessionController, ReplacementSessionSource},
    policy::AppPolicy,
    provider_controller::ProviderController,
    runtime_builder::{
        build_compaction, build_runtime, configured_context_window, RuntimeBuildOptions,
    },
};

pub(crate) use super::interactive_run_controller::{
    SteeringAcceptanceFuture, SteeringRetractionFuture,
};
use super::interactive_state::{
    active_run_disposition, ActiveRunCommand, ActiveRunDisposition, InteractiveState,
};

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
    pub(crate) unavailable_error: Option<rho_providers::model::ModelError>,
}

pub(crate) struct InteractiveRuntime {
    runtime: Rho,
    runs: InteractiveRunController,
    sessions: InteractiveSessionController,
    provider: ProviderController,
    tools: AppToolSet,
    workspace: Workspace,
    system_prompt: SystemPrompt,
    compaction: CompactionConfig,
    context_window: Option<u64>,
    usage_recording: rho_sdk::ProviderRequestUsageRecording,
    permission_mode: PermissionMode,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    approval_receiver: Option<ApprovalRequestReceiver>,
    agent_id: String,
    agent_fingerprint: String,
    pending_persistence_error: Option<anyhow::Error>,
    pending_persistence_checkpoint: Option<(StoredSession, rho_sdk::SessionSnapshot)>,
    /// True after the current provider completes a live turn on the current history.
    live_context_warm: bool,
}

enum TurnPrelude {
    None,
    ToolCall(ToolCall),
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
                    rho_providers::auth::provider_credentials::ApplicationCredentialSource::new(
                        Arc::new(AppCredentialStore),
                    );
                build_sdk_provider_with_source(sdk_options.provider.clone(), &credentials)?
            }
        };
        let mut capabilities = agent.capabilities().clone();
        if no_subagents {
            capabilities.remove(&ToolCapability::Agent);
            capabilities.remove(&ToolCapability::Agents);
        }
        if !questionnaire_enabled {
            capabilities.remove(&ToolCapability::Questionnaire);
        }
        let launch_delegation_enabled = capabilities.contains(&ToolCapability::Agent);
        let delegation_enabled =
            launch_delegation_enabled || capabilities.contains(&ToolCapability::Agents);
        let tools = if no_tools {
            AppToolSet::disabled()
        } else {
            let mut tool_options = ToolSetOptions::new(capabilities);
            if delegation_enabled {
                tool_options = tool_options.delegation(DelegationConfig::new(
                    cwd.clone(),
                    config_path,
                    BackgroundSubagents::Enabled,
                ));
            }
            AppToolSet::new(config, diagnostics.clone(), tool_options)
        };
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
        let usage_recording = crate::usage::default_recording().await;
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
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: usage_recording.clone(),
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
            runs: InteractiveRunController::default(),
            sessions: InteractiveSessionController::new(session, storage),
            provider: ProviderController::new(provider, sdk_options.runtime.reasoning),
            tools,
            workspace,
            system_prompt,
            compaction,
            context_window,
            usage_recording,
            permission_mode,
            approval_handler,
            approval_receiver,
            agent_id,
            agent_fingerprint,
            pending_persistence_error: None,
            pending_persistence_checkpoint: None,
            live_context_warm: false,
        })
    }

    pub(crate) fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }

    /// Returns whether a model run is active on the interactive run controller.
    ///
    /// This is the authoritative lifecycle signal for provider turns. TUI
    /// `App.running` can also be true for UI-only work such as compaction.
    pub(crate) fn is_run_active(&self) -> bool {
        self.runs.is_active()
    }

    /// Rebuilds the SDK runtime so the requested permission mode applies to the next turn.
    pub(crate) async fn set_permission_mode(&mut self, mode: PermissionMode) -> anyhow::Result<()> {
        if self.runs.is_active() {
            anyhow::bail!("permission mode cannot change while a run is active");
        }
        if self.permission_mode == mode {
            return Ok(());
        }

        let snapshot = self.sessions.session().snapshot();
        let (approval_handler, approval_receiver) = approval_channel_for(mode);
        let replacement_runtime = build_runtime(RuntimeBuildOptions {
            provider: Arc::clone(self.provider.provider()),
            tools: self.tools.tools(),
            workspace: self.workspace.clone(),
            workspace_policy: AppPolicy::for_mode(mode),
            approval_handler: approval_handler.clone(),
            system_prompt: self.system_prompt.clone(),
            reasoning: self.provider.reasoning(),
            compaction: self.compaction.clone(),
            context_window: self.context_window,
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: self.usage_recording.clone(),
        })?;
        let replacement_session = replacement_runtime
            .session(SessionOptions::from_snapshot(snapshot))
            .await?;

        let previous_runtime = std::mem::replace(&mut self.runtime, replacement_runtime);
        self.sessions.replace_runtime_session(replacement_session);
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
        self.sessions.history()
    }

    pub(crate) fn can_compact(&self) -> bool {
        self.can_compact_messages(&self.sessions.history())
    }

    pub(crate) fn can_compact_messages(&self, messages: &[Message]) -> bool {
        let target_tokens = self
            .context_window
            .map(|window| self.compaction.target_tokens(window))
            .unwrap_or(u64::MAX / 2);
        crate::compaction::partition_messages_for_compaction(
            messages,
            &self.tools.specs(),
            target_tokens,
        )
        .is_some()
    }

    pub(crate) fn provider_identity(&self) -> rho_sdk::model::ModelIdentity {
        self.provider.provider().identity()
    }

    pub(crate) fn provider_context_omissions(
        &self,
        target: &rho_sdk::model::ModelIdentity,
    ) -> rho_sdk::model::handoff::HandoffReport {
        rho_sdk::model::handoff::report_message_omissions(&self.sessions.history(), target)
    }

    pub(crate) fn live_context_warm(&self) -> bool {
        self.live_context_warm
    }

    pub(crate) fn mark_live_context_warm(&mut self) {
        self.live_context_warm = true;
    }

    fn invalidate_live_context(&mut self) {
        self.live_context_warm = false;
    }

    pub(crate) fn take_pending_omission(
        &mut self,
    ) -> Option<rho_sdk::model::handoff::HandoffReport> {
        self.sessions.take_pending_omission()
    }

    pub(crate) fn session_id(&self) -> &SessionId {
        self.sessions.id()
    }

    pub(crate) fn usage_recording(&self) -> rho_sdk::ProviderRequestUsageRecording {
        self.usage_recording.clone()
    }

    pub(crate) fn workspace_path(&self) -> &std::path::Path {
        self.workspace.root()
    }

    pub(crate) fn set_context_window(&mut self, context_window: Option<u64>) {
        self.context_window = context_window;
        if !self.runs.is_active() {
            let _ = self.refresh_compaction();
        }
    }

    pub(crate) fn take_context_usage(&mut self) -> Option<rho_sdk::model::ContextUsage> {
        self.runs.take_context_usage()
    }

    /// Warnings queued while the TUI owns the terminal (e.g. resume omissions).
    pub(crate) fn take_notices(&mut self) -> Vec<String> {
        self.sessions.take_notices()
    }

    pub(crate) fn agent_identity(&self) -> (&str, &str) {
        (&self.agent_id, &self.agent_fingerprint)
    }

    pub(crate) fn attach_storage(&mut self, storage: StoredSession) {
        self.sessions.attach_storage(storage);
    }

    pub(crate) async fn start(
        &mut self,
        input: UserInput,
        display_user: Option<Message>,
    ) -> Result<(), Error> {
        self.start_run(input, display_user, TurnPrelude::None).await
    }

    pub(crate) async fn start_with_tool_call(
        &mut self,
        input: UserInput,
        display_user: Option<Message>,
        tool_call: ToolCall,
    ) -> Result<(), Error> {
        self.start_run(input, display_user, TurnPrelude::ToolCall(tool_call))
            .await
    }

    async fn start_run(
        &mut self,
        input: UserInput,
        display_user: Option<Message>,
        prelude: TurnPrelude,
    ) -> Result<(), Error> {
        if self.runs.state() != InteractiveState::Idle {
            return Err(Error::SessionBusy);
        }
        if let Some(source) = self.sessions.pending_replacement() {
            self.rebuild_session(source)
                .await
                .map_err(|error| Error::Persistence {
                    message: error.to_string(),
                })?;
        }
        let model_user = Message::User(input.blocks().to_vec());
        let mut request_history = self.sessions.history();
        let pending_turn = PendingTurn::new(model_user, display_user, request_history.len());
        request_history.push(Message::User(input.blocks().to_vec()));
        let context_usage = rho_sdk::model::ContextUsage::estimated(
            rho_sdk::model::context::estimate_context_tokens(&request_history, &self.tools.specs()),
            self.context_window,
        );
        let run = match prelude {
            TurnPrelude::None => self.sessions.session().start(input).await?,
            TurnPrelude::ToolCall(call) => {
                self.sessions
                    .session()
                    .start_with_tool_call(input, call)
                    .await?
            }
        };
        self.runs.begin(run, pending_turn, context_usage)
    }

    pub(crate) async fn next_event(&mut self) -> Option<RunEvent> {
        let event = self.runs.next_event(self.context_window).await;
        if let Some(RunEvent::CompactionCompleted { outcome, .. }) = &event {
            let snapshot = outcome.committed_snapshot().ok_or_else(|| {
                anyhow::anyhow!("automatic compaction event is missing its committed snapshot")
            });
            let checkpoint = self.capture_durable_session();
            let display_user = self
                .runs
                .pending_turn()
                .map(|turn| turn.display_user().unwrap_or_else(|| turn.model_user()));
            match (checkpoint, snapshot) {
                (Ok(checkpoint), Ok(snapshot)) => {
                    if let Err(error) =
                        self.sessions
                            .save_automatic_compaction(snapshot, display_user, outcome)
                    {
                        self.runs.cancel();
                        self.pending_persistence_error = Some(error);
                        self.pending_persistence_checkpoint = checkpoint;
                    }
                }
                (Err(error), _) | (_, Err(error)) => {
                    self.runs.cancel();
                    self.pending_persistence_error = Some(error);
                }
            }
        }
        event
    }

    pub(crate) fn cancel(&mut self) {
        self.runs.cancel();
    }

    pub(crate) fn request_steer(
        &mut self,
        input: UserInput,
    ) -> Result<SteeringAcceptanceFuture, Error> {
        self.runs.request_steer(input)
    }

    pub(crate) fn request_steering_retraction(
        &self,
        id: rho_sdk::SteeringId,
    ) -> Result<SteeringRetractionFuture, Error> {
        self.runs.request_steering_retraction(id)
    }

    pub(crate) async fn respond(
        &mut self,
        request_id: HostInputId,
        response: HostInputResponse,
    ) -> Result<(), Error> {
        self.runs.respond(request_id, response).await
    }

    pub(crate) async fn finish_run(&mut self) -> anyhow::Result<RunOutcome> {
        let finished = self.runs.finish().await;
        if let Some(error) = self.pending_persistence_error.take() {
            let checkpoint = self.pending_persistence_checkpoint.take();
            let rollback = self.restore_durable_session(checkpoint).await;
            return match rollback {
                Ok(()) => Err(anyhow::anyhow!(
                    "could not persist automatic compaction: {error}"
                )),
                Err(rollback_error) => Err(anyhow::anyhow!(
                    "could not persist automatic compaction: {error}; could not restore durable state: {rollback_error}"
                )),
            };
        }
        let finished = finished?;
        let checkpoint = self.capture_durable_session();
        if let Err(error) = self.sessions.sync_finished_turn(
            finished.pending_turn.as_ref(),
            finished.outcome.as_ref().ok(),
        ) {
            let (checkpoint, capture_error) = match checkpoint {
                Ok(checkpoint) => (checkpoint, None),
                Err(capture_error) => (None, Some(capture_error)),
            };
            let rollback = self.restore_durable_session(checkpoint).await;
            return match (capture_error, rollback) {
                (None, Ok(())) => Err(error),
                (Some(capture_error), Ok(())) => Err(anyhow::anyhow!(
                    "{error}; could not capture rollback checkpoint: {capture_error}"
                )),
                (None, Err(rollback_error)) => Err(anyhow::anyhow!(
                    "{error}; could not restore durable state: {rollback_error}"
                )),
                (Some(capture_error), Err(rollback_error)) => Err(anyhow::anyhow!(
                    "{error}; could not capture rollback checkpoint: {capture_error}; could not restore durable state: {rollback_error}"
                )),
            };
        }
        self.refresh_context_usage();
        Ok(finished.outcome?)
    }

    pub(crate) async fn compact(&mut self) -> anyhow::Result<bool> {
        if self.runs.is_active() {
            anyhow::bail!("session is busy");
        }
        let checkpoint = self.capture_durable_session()?;
        let outcome = self.sessions.session().compact().await?;
        if let Err(error) = self.sessions.save_compaction_snapshot(&[], &outcome) {
            let rollback = self.restore_durable_session(checkpoint).await;
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(anyhow::anyhow!(
                    "{error}; could not restore durable state: {rollback_error}"
                )),
            };
        }
        let reduced = outcome.current_messages() < outcome.previous_messages();
        if reduced {
            self.runs.note_manual_compaction(self.context_window);
            self.invalidate_live_context();
        }
        Ok(reduced)
    }

    pub(crate) fn reset(&mut self) -> anyhow::Result<()> {
        if self.runs.is_active() {
            anyhow::bail!("cannot reset while a run is active");
        }
        let session_id = self.sessions.reset()?;
        if let Some(manager) = self.tools.subagents() {
            manager.set_session(session_id.to_string());
        }
        self.invalidate_live_context();
        Ok(())
    }

    pub(crate) async fn resume(
        &mut self,
        storage: StoredSession,
        _history: Vec<Message>,
    ) -> anyhow::Result<()> {
        if self.runs.is_active() {
            debug_assert_eq!(
                active_run_disposition(ActiveRunCommand::SwitchSession),
                ActiveRunDisposition::RejectUntilFinished
            );
            anyhow::bail!("cannot switch sessions while a run is active");
        }
        let id = storage.id().to_string();
        self.rebuild_session(ReplacementSessionSource::Snapshot {
            storage: storage.clone(),
            id,
        })
        .await?;
        if let Some(manager) = self.tools.subagents() {
            manager.set_session(self.sessions.session().id().to_string());
        }
        self.sessions.set_resumed_storage(storage);
        self.invalidate_live_context();
        Ok(())
    }

    pub(crate) fn stored_session(&self) -> Option<StoredSession> {
        self.sessions.storage().cloned()
    }

    pub(crate) async fn select_tree_node(
        &mut self,
        storage: StoredSession,
        target_id: &crate::session::tree::NodeId,
    ) -> anyhow::Result<()> {
        if self.runs.is_active() {
            anyhow::bail!("cannot navigate the session tree while a run is active");
        }
        let identity = self.provider.provider().identity();
        let id = storage.id().to_string();
        let snapshot =
            storage.snapshot_for_node(target_id, identity.clone(), prompt_cache_key(&id))?;
        let resume_omission = resume_omissions_report(&snapshot, &identity);
        let replacement_runtime = build_runtime(RuntimeBuildOptions {
            provider: Arc::clone(self.provider.provider()),
            tools: self.tools.tools(),
            workspace: self.workspace.clone(),
            workspace_policy: AppPolicy::for_mode(self.permission_mode),
            approval_handler: self.approval_handler.clone(),
            system_prompt: self.system_prompt.clone(),
            reasoning: self.provider.reasoning(),
            compaction: self.compaction.clone(),
            context_window: self.context_window,
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: self.usage_recording.clone(),
        })?;
        let replacement_session = replacement_runtime
            .session(SessionOptions::from_snapshot(snapshot))
            .await?;

        // Do not change the live runtime until the selected leaf is durable.
        if let Err(error) = storage.set_leaf(target_id) {
            replacement_runtime.shutdown();
            return Err(error);
        }
        let previous_runtime = std::mem::replace(&mut self.runtime, replacement_runtime);
        self.sessions
            .replace_session(replacement_session, resume_omission);
        self.sessions.set_resumed_storage(storage);
        previous_runtime.shutdown();
        self.invalidate_live_context();
        self.refresh_context_usage();
        Ok(())
    }

    pub(crate) fn replace_provider(
        &mut self,
        provider: Arc<dyn ModelProvider>,
        reasoning: rho_sdk::ReasoningLevel,
    ) -> Result<rho_sdk::model::handoff::HandoffReport, Error> {
        if self.runs.is_active() {
            debug_assert_eq!(
                active_run_disposition(ActiveRunCommand::ReplaceProvider),
                ActiveRunDisposition::DeferUntilFinished
            );
            return Err(Error::SessionBusy);
        }
        self.runs.begin_provider_switch()?;
        let report = match self
            .provider
            .replace(self.sessions.session(), provider, reasoning)
        {
            Ok(report) => report,
            Err(error) => {
                self.runs.finish_transition();
                return Err(error);
            }
        };
        if let Err(error) = self.refresh_compaction() {
            self.runs.finish_transition();
            return Err(error);
        }
        let identity = self.provider.provider().identity();
        if let Some(manager) = self.tools.subagents() {
            manager.update_model(&identity.provider, &identity.model, reasoning);
        }
        self.invalidate_live_context();
        self.runs.finish_transition();
        Ok(report)
    }

    fn refresh_compaction(&mut self) -> Result<(), Error> {
        let (compactor, policy) = build_compaction(
            Arc::clone(self.provider.provider()),
            self.tools.tools(),
            self.provider.reasoning(),
            self.compaction.clone(),
            self.context_window,
            self.usage_recording.clone(),
        );
        self.sessions
            .session_mut()
            .set_compaction(Some(Arc::new(compactor)), policy)
    }

    fn refresh_context_usage(&mut self) {
        self.runs
            .note_context_usage(rho_sdk::model::ContextUsage::estimated(
                rho_sdk::model::context::estimate_context_tokens(
                    &self.sessions.history(),
                    &self.tools.specs(),
                ),
                self.context_window,
            ));
    }

    pub(crate) fn append_user_context_with_display(
        &mut self,
        model: String,
        display: String,
    ) -> anyhow::Result<()> {
        self.sessions
            .session()
            .append_message(Message::user_text(model))?;
        self.sessions
            .save_snapshot(&[Message::user_text(display)])?;
        self.refresh_context_usage();
        Ok(())
    }

    pub(crate) async fn shutdown(&mut self) {
        if self.runs.is_active() {
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

    pub(crate) fn has_tool(&self, name: &str) -> bool {
        self.tools.contains(name)
    }

    pub(crate) fn subagents(&self) -> Option<&crate::tools::agent::SubagentManager> {
        self.tools.subagents()
    }

    #[cfg(test)]
    fn observe_event(&mut self, event: &RunEvent) {
        self.runs.observe_event(event, self.context_window);
    }

    fn capture_durable_session(
        &self,
    ) -> anyhow::Result<Option<(StoredSession, rho_sdk::SessionSnapshot)>> {
        let Some(storage) = self.sessions.storage().cloned() else {
            return Ok(None);
        };
        let id = storage.id().to_string();
        let snapshot = storage
            .snapshot_for_resume(self.provider.provider().identity(), prompt_cache_key(&id))?;
        Ok(Some((storage, snapshot)))
    }

    async fn restore_durable_session(
        &mut self,
        checkpoint: Option<(StoredSession, rho_sdk::SessionSnapshot)>,
    ) -> anyhow::Result<()> {
        if let Some((storage, snapshot)) = checkpoint {
            self.rebuild_session(ReplacementSessionSource::DurableSnapshot { snapshot })
                .await?;
            self.sessions.set_resumed_storage(storage);
            return Ok(());
        }
        let storage = self
            .sessions
            .storage()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("durable session storage is unavailable"))?;
        let id = storage.id().to_string();
        self.rebuild_session(ReplacementSessionSource::Snapshot {
            storage: storage.clone(),
            id,
        })
        .await?;
        self.sessions.set_resumed_storage(storage);
        Ok(())
    }

    async fn rebuild_session(&mut self, source: ReplacementSessionSource) -> anyhow::Result<()> {
        let identity = self.provider.provider().identity();
        let (options, resume_omission) = match source {
            ReplacementSessionSource::DurableSnapshot { snapshot } => {
                let omission = resume_omissions_report(&snapshot, &identity);
                (SessionOptions::from_snapshot(snapshot), omission)
            }
            ReplacementSessionSource::Snapshot { storage, id } => {
                let snapshot =
                    storage.snapshot_for_resume(identity.clone(), prompt_cache_key(&id))?;
                let omission = resume_omissions_report(&snapshot, &identity);
                (SessionOptions::from_snapshot(snapshot), omission)
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
            provider: Arc::clone(self.provider.provider()),
            tools: self.tools.tools(),
            workspace: self.workspace.clone(),
            workspace_policy: AppPolicy::for_mode(self.permission_mode),
            approval_handler: self.approval_handler.clone(),
            system_prompt: self.system_prompt.clone(),
            reasoning: self.provider.reasoning(),
            compaction: self.compaction.clone(),
            context_window: self.context_window,
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: self.usage_recording.clone(),
        })?;
        let replacement_session = replacement_runtime.session(options).await?;
        let previous_runtime = std::mem::replace(&mut self.runtime, replacement_runtime);
        self.sessions
            .replace_session(replacement_session, resume_omission);
        previous_runtime.shutdown();
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
    rho_providers::providers::openai::prompt_cache_key_from_session_id(id)
        .unwrap_or_else(|| format!("rho:{id}"))
}

fn resume_omissions_report(
    snapshot: &rho_sdk::SessionSnapshot,
    target: &rho_sdk::model::ModelIdentity,
) -> Option<rho_sdk::model::handoff::HandoffReport> {
    let report = snapshot.provider_context_omissions(target);
    report.has_omissions().then_some(report)
}

fn resume_omissions_notice(
    snapshot: &rho_sdk::SessionSnapshot,
    target: &rho_sdk::model::ModelIdentity,
) -> Option<String> {
    resume_omissions_report(snapshot, target).map(|report| {
        format!(
            "omitted {} incompatible provider-native context block(s) while resuming session (kinds: {})",
            report.omitted_provider_context,
            report.omitted_kinds.join(", ")
        )
    })
}

#[cfg(test)]
#[path = "interactive_runtime_tests.rs"]
mod tests;
