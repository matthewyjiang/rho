use std::{path::PathBuf, sync::Arc};

use rho_sdk::{
    model::{Message, ToolCall},
    provider::ModelProvider,
    ApprovalHandler, ApprovalRequestReceiver, Error, HostInputId, HostInputResponse, Rho, RunEvent,
    RunOutcome, SessionId, SessionOptions, SystemPrompt, UserInput, Workspace,
};

use {
    crate::compaction::CompactionConfig,
    crate::config::Config,
    crate::diagnostics::RuntimeDiagnostics,
    crate::permission::PermissionMode,
    crate::session::Session as StoredSession,
    crate::tools::{agent::BackgroundSubagents, sdk_registry::AppToolSet},
};

#[path = "interactive_runtime_hooks.rs"]
mod session_hooks;
#[path = "interactive_runtime_startup.rs"]
mod startup;
#[path = "interactive_runtime_workspace_rewind.rs"]
mod workspace_rewind;

use super::{
    agent_binding::BoundAgent,
    interactive_run_controller::{InteractiveRunController, PendingTurn},
    interactive_session_controller::{InteractiveSessionController, ReplacementSessionSource},
    policy::AppPolicy,
    provider_controller::ProviderController,
    runtime_builder::{
        build_compaction, build_runtime, configured_context_window, RuntimeBuildOptions,
    },
    tools_prompt::{assemble_tools_and_prompt, ToolsAndPromptOptions},
};

pub(crate) use super::interactive_run_controller::{
    SteeringAcceptanceFuture, SteeringRetractionFuture,
};
use super::interactive_state::{
    active_run_disposition, ActiveRunCommand, ActiveRunDisposition, InteractiveState,
};
use startup::{
    approval_channel_for, prompt_cache_key, resolve_provider, resolve_session_options,
    resume_omissions_report,
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
    /// One hook pipeline for the whole session. Runtime rebuilds reattach it
    /// rather than starting a second worker.
    hooks: Option<crate::hooks::HookPipeline>,
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
    experimental_workspace_rewind: bool,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    approval_receiver: Option<ApprovalRequestReceiver>,
    agent: BoundAgent,
    agent_id: String,
    agent_fingerprint: String,
    pending_persistence_error: Option<anyhow::Error>,
    pending_persistence_checkpoint: Option<(StoredSession, rho_sdk::SessionSnapshot)>,
    /// True after the current provider completes a live turn on the current history.
    live_context_warm: bool,
    /// Runs that reached a terminal outcome, reported at the session boundary.
    completed_runs: u64,
}

fn bind_subagent_parent(
    tools: &AppToolSet,
    session_id: &SessionId,
    storage: Option<&StoredSession>,
) {
    if let Some(manager) = tools.subagents() {
        manager.bind_parent_session(crate::subagent::RunPlacement::for_parent_session(
            session_id.to_string(),
            storage.and_then(StoredSession::subagents_dir),
        ));
    }
    tools
        .workflow_tracker()
        .bind_parent_session(session_id.to_string());
}

enum TurnPrelude {
    None,
    ToolCall(ToolCall),
}

#[derive(Clone, Copy)]
enum ReplacementLifecycle {
    Started,
    Rebound,
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
        let provider = resolve_provider(unavailable_error, &sdk_options)?;
        let (tools, system_prompt) = assemble_tools_and_prompt(ToolsAndPromptOptions {
            config,
            config_path,
            cwd: &cwd,
            no_system_prompt,
            no_tools,
            no_subagents,
            questionnaire_enabled,
            background_subagents: BackgroundSubagents::Enabled,
            diagnostics: &diagnostics,
            agent: &agent,
        })?;
        let workspace = sdk_options.workspace.build_workspace()?;
        let context_window = configured_context_window(config);
        let compaction = sdk_options.runtime.compaction.clone();
        let permission_mode = config.permission_mode;
        let (approval_handler, approval_receiver) = approval_channel_for(permission_mode);
        diagnostics.update_compaction_config(&compaction);
        let usage_recording = crate::usage::default_recording().await;
        let hooks = crate::hooks::start_for_cwd(&cwd);
        if let Some(hooks) = hooks.as_ref() {
            diagnostics.attach_hooks(hooks);
        }
        let runtime = build_runtime(RuntimeBuildOptions {
            provider: Arc::clone(&provider),
            tools: tools.tools(),
            workspace: workspace.clone(),
            workspace_policy: AppPolicy::for_mode(permission_mode),
            approval_session: approval_handler
                .clone()
                .map(rho_sdk::ApprovalSession::from_shared),
            system_prompt: system_prompt.clone(),
            reasoning: sdk_options.runtime.reasoning,
            service_tier: sdk_options.runtime.service_tier,
            compaction: compaction.clone(),
            context_window,
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: usage_recording.clone(),
            hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
            hooks: hooks.as_ref(),
        })?;
        let session_options =
            resolve_session_options(&provider, history, session_id.as_deref(), storage.as_ref())?;
        let session = runtime.session(session_options).await?;
        bind_subagent_parent(&tools, session.id(), storage.as_ref());
        Ok(Self {
            runtime,
            hooks,
            runs: InteractiveRunController::default(),
            sessions: InteractiveSessionController::new(
                session,
                storage,
                tools.web_access().clone(),
            ),
            provider: ProviderController::new(provider, sdk_options.runtime.reasoning),
            tools,
            workspace,
            system_prompt,
            compaction,
            context_window,
            usage_recording,
            permission_mode,
            experimental_workspace_rewind: config.experimental_workspace_rewind,
            approval_handler,
            approval_receiver,
            agent,
            agent_id,
            agent_fingerprint,
            pending_persistence_error: None,
            pending_persistence_checkpoint: None,
            live_context_warm: false,
            completed_runs: 0,
        })
    }

    pub(crate) fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }

    pub(crate) fn workspace_rewind_enabled(&self) -> bool {
        self.experimental_workspace_rewind
    }

    pub(crate) fn fast_mode(&self) -> bool {
        self.sessions.session().service_tier() == Some(rho_sdk::model::ServiceTier::Priority)
    }

    pub(crate) fn set_fast_mode(&self, enabled: bool) -> anyhow::Result<()> {
        self.sessions
            .session()
            .set_service_tier(enabled.then_some(rho_sdk::model::ServiceTier::Priority))?;
        Ok(())
    }

    /// Returns whether a model run is active on the interactive run controller.
    ///
    /// Prefer this for provider-lifecycle decisions. TUI busy UI uses
    /// `SessionUiPhase` (`App::is_ui_busy`) because compaction blocks the UI
    /// without an active provider run.
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
            approval_session: approval_handler
                .clone()
                .map(rho_sdk::ApprovalSession::from_shared),
            system_prompt: self.system_prompt.clone(),
            reasoning: self.provider.reasoning(),
            service_tier: self.sessions.session().service_tier(),
            compaction: self.compaction.clone(),
            context_window: self.context_window,
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: self.usage_recording.clone(),
            hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
            hooks: self.hooks.as_ref(),
        })?;
        let replacement_session = replacement_runtime
            .rebind_session(SessionOptions::from_snapshot(snapshot))
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

    /// Rebuilds compaction against the new window so a failure surfaces instead
    /// of leaving the session compacting for the previous model's limits.
    ///
    /// Commits `context_window` only after compaction refresh succeeds when the
    /// runtime is idle. Active runs defer the rebuild and store the value now.
    pub(crate) fn set_context_window(&mut self, context_window: Option<u64>) -> Result<(), Error> {
        if self.runs.is_active() {
            self.context_window = context_window;
            return Ok(());
        }
        let previous = self.context_window;
        self.context_window = context_window;
        if let Err(error) = self.refresh_compaction() {
            self.context_window = previous;
            return Err(error);
        }
        Ok(())
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

    pub(crate) fn bound_definition(&self) -> &crate::agent::AgentDefinition {
        self.agent.definition()
    }

    pub(crate) fn attach_storage(&mut self, storage: StoredSession) {
        bind_subagent_parent(&self.tools, self.sessions.session().id(), Some(&storage));
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
            self.rebuild_session(source, ReplacementLifecycle::Started)
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
        self.tools
            .checkpoint_tracker()
            .begin_turn(self.sessions.storage())
            .map_err(|error| Error::Persistence {
                message: error.to_string(),
            })?;
        let run_result = match prelude {
            TurnPrelude::None => self.sessions.session().start(input).await,
            TurnPrelude::ToolCall(call) => {
                self.sessions
                    .session()
                    .start_with_tool_call(input, call)
                    .await
            }
        };
        let run = match run_result {
            Ok(run) => run,
            Err(error) => {
                self.tools.checkpoint_tracker().discard_turn();
                return Err(error);
            }
        };
        if let Err(error) = self.runs.begin(run, pending_turn, context_usage) {
            self.tools.checkpoint_tracker().discard_turn();
            return Err(error);
        }
        Ok(())
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
            self.tools.checkpoint_tracker().discard_turn();
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
        let finished = match finished {
            Ok(finished) => finished,
            Err(error) => {
                self.tools.checkpoint_tracker().discard_turn();
                return Err(error);
            }
        };
        let checkpoint = self.capture_durable_session();
        if let Err(error) = self.sessions.sync_finished_turn(
            finished.pending_turn.as_ref(),
            finished.outcome.as_ref().ok(),
        ) {
            self.tools.checkpoint_tracker().discard_turn();
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
        if let Some(storage) = self.sessions.storage() {
            let outcome = match finished.outcome.as_ref() {
                Ok(_) => crate::session::workspace_checkpoint::CheckpointOutcome::Completed,
                Err(Error::Cancelled | Error::Interrupted { .. }) => {
                    crate::session::workspace_checkpoint::CheckpointOutcome::Cancelled
                }
                Err(_) => crate::session::workspace_checkpoint::CheckpointOutcome::Failed,
            };
            match storage.active_checkpoint_target() {
                Ok(Some((node_id, revision))) => {
                    if let Err(error) = self
                        .tools
                        .checkpoint_tracker()
                        .finalize_turn(node_id, revision, outcome)
                    {
                        tracing::warn!(%error, "failed to persist workspace checkpoint");
                        self.tools.checkpoint_tracker().discard_turn();
                    }
                }
                Ok(None) => self.tools.checkpoint_tracker().discard_turn(),
                Err(error) => {
                    tracing::warn!(%error, "failed to resolve workspace checkpoint target");
                    self.tools.checkpoint_tracker().discard_turn();
                }
            }
        } else {
            self.tools.checkpoint_tracker().discard_turn();
        }
        self.refresh_context_usage();
        self.completed_runs = self.completed_runs.saturating_add(1);
        Ok(finished.outcome?)
    }

    pub(crate) async fn compact(&mut self) -> anyhow::Result<Option<rho_sdk::CompactionOutcome>> {
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
        let reduced = outcome.current_messages() < outcome.previous_messages()
            || outcome.removed_tokens() > 0;
        if reduced {
            self.runs.note_manual_compaction(self.context_window);
            self.invalidate_live_context();
            Ok(Some(outcome))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn reset(&mut self) -> anyhow::Result<()> {
        if self.runs.is_active() {
            anyhow::bail!("cannot reset while a run is active");
        }
        self.runtime
            .hooks()
            .session_completed(self.sessions.session().id(), self.completed_runs);
        self.completed_runs = 0;
        let session_id = self.sessions.reset()?;
        bind_subagent_parent(&self.tools, &session_id, None);
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
        self.runtime
            .hooks()
            .session_completed(self.sessions.session().id(), self.completed_runs);
        self.completed_runs = 0;
        let id = storage.id().to_string();
        self.rebuild_session(
            ReplacementSessionSource::Snapshot {
                storage: storage.clone(),
                id,
            },
            ReplacementLifecycle::Started,
        )
        .await?;
        bind_subagent_parent(&self.tools, self.sessions.session().id(), Some(&storage));
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
            approval_session: self
                .approval_handler
                .clone()
                .map(rho_sdk::ApprovalSession::from_shared),
            system_prompt: self.system_prompt.clone(),
            reasoning: self.provider.reasoning(),
            service_tier: self.sessions.session().service_tier(),
            compaction: self.compaction.clone(),
            context_window: self.context_window,
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: self.usage_recording.clone(),
            hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
            hooks: self.hooks.as_ref(),
        })?;
        let replacement_session = replacement_runtime
            .rebind_session(SessionOptions::from_snapshot(snapshot))
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
        auth: &str,
    ) -> Result<rho_sdk::model::handoff::HandoffReport, Error> {
        if self.runs.is_active() {
            debug_assert_eq!(
                active_run_disposition(ActiveRunCommand::ReplaceProvider),
                ActiveRunDisposition::DeferUntilFinished
            );
            return Err(Error::SessionBusy);
        }
        self.runs.begin_provider_switch()?;
        // Capture prior identity so post-replace failures can roll back and keep
        // `Err` meaning "active provider unchanged" for callers.
        let previous_provider = Arc::clone(self.provider.provider());
        let previous_reasoning = self.provider.reasoning();
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
            if let Err(rollback_error) = self.provider.replace(
                self.sessions.session(),
                previous_provider,
                previous_reasoning,
            ) {
                self.runs.finish_transition();
                return Err(Error::InvalidConfiguration {
                    message: format!(
                        "{error}; also failed to restore the previous provider: {rollback_error}"
                    ),
                });
            }
            self.runs.finish_transition();
            return Err(error);
        }
        let identity = self.provider.provider().identity();
        if let Some(manager) = self.tools.subagents() {
            manager.update_selection(&identity.provider, &identity.model, reasoning, auth);
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
        self.drain_hooks().await;
        self.tools.shutdown().await;
    }

    pub(crate) fn has_tool(&self, name: &str) -> bool {
        self.tools.contains(name)
    }

    /// Returns the tool ceiling for workflow agents started by this session.
    pub(crate) fn workflow_host_capabilities(&self) -> crate::agent::AgentCapabilities {
        crate::agent::AgentCapabilities::new(
            self.tools
                .unfiltered_names()
                .map(crate::agent::ToolCapability::parse)
                .collect(),
        )
    }

    pub(crate) fn subagents(&self) -> Option<&crate::tools::agent::SubagentManager> {
        self.tools.subagents()
    }

    pub(crate) fn workflow_tracker(&self) -> &crate::tools::workflow_tracker::WorkflowRunTracker {
        self.tools.workflow_tracker()
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
            self.rebuild_session(
                ReplacementSessionSource::DurableSnapshot { snapshot },
                ReplacementLifecycle::Rebound,
            )
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
        self.rebuild_session(
            ReplacementSessionSource::Snapshot {
                storage: storage.clone(),
                id,
            },
            ReplacementLifecycle::Rebound,
        )
        .await?;
        self.sessions.set_resumed_storage(storage);
        Ok(())
    }

    async fn rebuild_session(
        &mut self,
        source: ReplacementSessionSource,
        lifecycle: ReplacementLifecycle,
    ) -> anyhow::Result<()> {
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
            approval_session: self
                .approval_handler
                .clone()
                .map(rho_sdk::ApprovalSession::from_shared),
            system_prompt: self.system_prompt.clone(),
            reasoning: self.provider.reasoning(),
            service_tier: self.sessions.session().service_tier(),
            compaction: self.compaction.clone(),
            context_window: self.context_window,
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: self.usage_recording.clone(),
            hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
            hooks: self.hooks.as_ref(),
        })?;
        let replacement_session = match lifecycle {
            ReplacementLifecycle::Started => replacement_runtime.session(options).await?,
            ReplacementLifecycle::Rebound => replacement_runtime.rebind_session(options).await?,
        };
        let previous_runtime = std::mem::replace(&mut self.runtime, replacement_runtime);
        self.sessions
            .replace_session(replacement_session, resume_omission);
        previous_runtime.shutdown();
        Ok(())
    }
}

#[cfg(test)]
#[path = "interactive_runtime_tests.rs"]
mod tests;
