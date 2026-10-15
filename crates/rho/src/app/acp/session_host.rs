use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64},
        Arc, Mutex,
    },
};

use agent_client_protocol::{
    schema::v1::{
        AvailableCommandsUpdate, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
        NewSessionResponse, PromptRequest, PromptResponse, RequestPermissionOutcome,
        SessionConfigOption, SessionId, SessionNotification, SessionUpdate,
        SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    },
    Error as AcpError,
};
use futures_util::stream::{FuturesUnordered, StreamExt};
use rho_providers::{
    credentials::available_auth_modes,
    model::{
        catalog::{self, ModelSelection},
        models_dev, ReasoningRequestSource,
    },
    provider::model_reference,
};
use rho_sdk::{
    model::Message, ApprovalRequestReceiver, CancellationToken, PendingApproval, RunEvent,
    SessionOptions, UserInput,
};

use super::{
    commands::{self, CommandScope, PromptAction},
    config_options::{self, CurrentModel},
    events::EventMapper,
    fast_mode, permission,
    signals::SessionSignals,
    thought_level, AcpClientPort, AcpStartup,
};
use crate::{
    app::{
        conversation_switch::{
            apply_conversation_switch, resolve_model_switch_reasoning, ConversationSwitch,
            SwitchNotice,
        },
        interactive_runtime::startup::{prompt_cache_key, session_options_for_id},
        session_assembly::BuiltSession,
    },
    compaction::CompactionConfig,
    config::Config,
    credential_store::{build_provider_from_config_ensuring_catalog, AppCredentialStore},
    herdr::{HerdrReporter, HerdrSession, HerdrState},
    permission::PermissionMode,
    session::Session as StoredSession,
};

#[path = "session_host_build.rs"]
mod build;
#[path = "session_host_compact.rs"]
mod compact;
#[path = "session_host_convert.rs"]
mod convert;
#[path = "session_host_mode.rs"]
mod mode;
#[path = "session_host_persist.rs"]
mod persist;
#[path = "session_host_workflow.rs"]
mod workflow;

use build::{build_session, mode_state, session_config};
use convert::{user_input_from_prompt, validate_session_cwd, workspace_cwd};
use mode::SessionPermission;
use persist::TurnCheckpoint;

pub(super) struct SessionHost {
    acp_session_id: SessionId,
    built: BuiltSession,
    stored: StoredSession,
    /// Saved with every snapshot so a later load restores them.
    added_dirs: crate::added_dirs::AddedDirs,
    permission: SessionPermission,
    auth: String,
    prompt_gate: Arc<PromptGate>,
    completed_runs: u64,
    permission_placeholders: AtomicU64,
    replaced: Arc<AtomicBool>,
    /// Set when a compaction rewrote live state but could not be saved.
    /// Later prompts are refused so the session never diverges further
    /// from its file; `session/load` starts a fresh host from storage.
    unsaved_compaction: bool,
    herdr: HerdrReporter,
    /// The todo checklist the client last received as an ACP `plan`.
    sent_plan: Vec<agent_client_protocol::schema::v1::PlanEntry>,
    /// Notice found during `session/load`, sent with the first prompt because
    /// hosts such as T3 Code discard live updates while a load is in flight.
    load_notice: Option<String>,
}

/// Carries cancellation for the session's current prompt. The agent already
/// serializes prompts with `try_lock` on the host slot, so this gate only has
/// to cover the window between prompt entry and the run's token existing.
/// While `Starting`, a cancel trips the prompt token that `begin` returned,
/// so command expansion can stop, and is replayed onto the run's token as
/// soon as `activate` receives it. `cancel` is a no-op while idle.
pub(super) struct PromptGate {
    slot: Mutex<PromptGateState>,
}

#[derive(Debug)]
enum PromptGateState {
    Idle,
    Starting(CancellationToken),
    Active(CancellationToken),
}

impl PromptGate {
    pub(super) fn new() -> Self {
        Self {
            slot: Mutex::new(PromptGateState::Idle),
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, PromptGateState> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Opens the starting window for a prompt, dropping any cancel that
    /// arrived for the previous one. The returned token trips on a cancel
    /// that lands before `activate`.
    pub(super) fn begin(&self) -> CancellationToken {
        let token = CancellationToken::new();
        *self.slot() = PromptGateState::Starting(token.clone());
        token
    }

    fn activate(&self, token: CancellationToken) {
        let mut slot = self.slot();
        if matches!(&*slot, PromptGateState::Starting(starting) if starting.is_cancelled()) {
            token.cancel();
        }
        *slot = PromptGateState::Active(token);
    }

    pub(super) fn finish(&self) {
        *self.slot() = PromptGateState::Idle;
    }

    pub(super) fn cancel(&self) {
        match &mut *self.slot() {
            PromptGateState::Idle => {}
            PromptGateState::Starting(token) | PromptGateState::Active(token) => token.cancel(),
        }
    }

    /// Whether a new prompt may wait for the host instead of being refused:
    /// the current prompt was cancelled, or none is running (the host is only
    /// held briefly by teardown or a setup request).
    pub(super) fn admits_queued_prompt(&self) -> bool {
        match &*self.slot() {
            PromptGateState::Idle => true,
            PromptGateState::Starting(token) | PromptGateState::Active(token) => {
                token.is_cancelled()
            }
        }
    }
}

struct PromptGuard(Arc<PromptGate>);

impl Drop for PromptGuard {
    fn drop(&mut self) {
        self.0.finish();
    }
}

impl SessionHost {
    pub(super) async fn create(
        startup: &AcpStartup,
        request: NewSessionRequest,
    ) -> anyhow::Result<(Self, NewSessionResponse)> {
        let cwd = validate_session_cwd(&request.cwd)?;
        let config = session_config(&startup.config, &request.mcp_servers);
        let sdk_id = rho_sdk::SessionId::new();
        let added_dirs = startup.added_dirs.clone();
        let assembly = build_session(startup, &config, cwd, &added_dirs, |_| {
            Ok(session_options_for_id(sdk_id.clone()))
        })
        .await?;
        let (built, permission) = split_assembly(assembly, config.permission_mode);
        let stored = match StoredSession::create_with_id(
            cwd,
            built.session.id().as_str(),
            startup.agent.id().as_str(),
            &startup.agent.fingerprint().to_string(),
        ) {
            Ok(stored) => stored,
            Err(error) => {
                built.teardown().await;
                return Err(error);
            }
        };
        let acp_session_id = SessionId::new(built.session.id().as_str());
        let host = Self::from_built(
            acp_session_id.clone(),
            built,
            stored,
            added_dirs,
            permission,
            startup.config.auth.clone(),
            startup.herdr.clone(),
        );
        let response = NewSessionResponse::new(acp_session_id)
            .modes(mode_state(host.permission.mode))
            .config_options(host.config_options(&startup.config));
        Ok((host, response))
    }

    pub(super) async fn load(
        startup: &AcpStartup,
        request: LoadSessionRequest,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<(Self, LoadSessionResponse)> {
        let cwd = workspace_cwd(&request.cwd, &startup.cwd);
        if !request.cwd.as_os_str().is_empty() {
            validate_session_cwd(cwd)?;
        }
        let config = session_config(&startup.config, &request.mcp_servers);
        let (stored, histories) =
            StoredSession::open_by_id_with_histories(cwd, request.session_id.0.as_ref())?;
        // ACP has no notice channel at load, so a missing saved directory
        // is logged and skipped, matching the interactive notice.
        let root = std::fs::canonicalize(cwd)?;
        let restored = crate::added_dirs::AddedDirs::from_storage(&stored, &root);
        for dir in &restored.missing {
            tracing::warn!(dir = %dir.display(), "could not restore added directory: not found");
        }
        let added_dirs = startup.added_dirs.union(&root, &restored.dirs);
        let assembly = build_session(startup, &config, cwd, &added_dirs, |provider| {
            let snapshot =
                stored.snapshot_for_resume(provider.identity(), prompt_cache_key(stored.id()))?;
            Ok(SessionOptions::from_snapshot(snapshot))
        })
        .await?;
        let (built, permission) = split_assembly(assembly, config.permission_mode);
        let replay_result =
            replay_display_history(&request.session_id, &histories.display, client).await;
        if let Err(error) = replay_result {
            built.teardown().await;
            return Err(error);
        }
        let load_notice = built
            .prompt_template
            .is_some()
            .then(|| {
                crate::app::model_prompt_metadata::change_notice(
                    &built.session.snapshot(),
                    built.prompt.loaded.as_ref(),
                )
            })
            .flatten();
        let mut host = Self::from_built(
            request.session_id,
            built,
            stored,
            added_dirs,
            permission,
            startup.config.auth.clone(),
            startup.herdr.clone(),
        );
        host.load_notice = load_notice;
        let response = LoadSessionResponse::new()
            .modes(mode_state(host.permission.mode))
            .config_options(host.config_options(&startup.config));
        Ok((host, response))
    }

    /// Runs one `session/prompt`: a slash command or a model turn. A
    /// command that cannot run fails with [`commands::CommandRejected`].
    /// `config_path` is the process config file, which workflow runs reload.
    pub(super) async fn prompt(
        &mut self,
        request: PromptRequest,
        client: &dyn AcpClientPort,
        process_config: &Config,
        config_path: &std::path::Path,
    ) -> anyhow::Result<PromptResponse> {
        if self.unsaved_compaction {
            anyhow::bail!(
                "could not continue session: its compacted context was not saved; load the session again"
            );
        }
        let input = user_input_from_prompt(&request.prompt)?;
        let starting = self.prompt_gate.begin();
        let _guard = PromptGuard(Arc::clone(&self.prompt_gate));
        let _ = self
            .herdr
            .report_state(
                HerdrState::Working,
                None,
                Some(&HerdrSession::without_resume(
                    self.acp_session_id.0.as_ref(),
                )),
            )
            .await;
        let home = crate::paths::home_dir();
        let scope = self.command_scope(home.as_deref(), process_config);
        // Expansion can wait on an MCP server, so a cancel drops it.
        let action = tokio::select! {
            biased;
            () = starting.cancelled() => None,
            action = commands::resolve(input.clone(), &scope) => Some(action),
        };
        let result = match action {
            None => Ok(PromptResponse::new(
                agent_client_protocol::schema::v1::StopReason::Cancelled,
            )),
            Some(Ok(PromptAction::Turn { input: model, call })) => {
                self.run_turn(&input, model, call, client).await
            }
            Some(Ok(PromptAction::Compact { instructions })) => {
                self.compact(instructions, client).await
            }
            Some(Ok(PromptAction::StartWorkflow(start))) => {
                self.start_workflow(&input, start, process_config, config_path, client)
                    .await
            }
            Some(Err(rejected)) => Err(rejected.into()),
        };
        let _ = self
            .herdr
            .report_state(
                HerdrState::Idle,
                None,
                Some(&HerdrSession::without_resume(
                    self.acp_session_id.0.as_ref(),
                )),
            )
            .await;
        result
    }

    pub(super) async fn set_config_option(
        &mut self,
        request: SetSessionConfigOptionRequest,
        process_config: &Config,
    ) -> Result<SetSessionConfigOptionResponse, AcpError> {
        match request.config_id.0.as_ref() {
            permission::MODE_CONFIG_ID => {
                let mode = permission::parse_mode_config_request(&request)?;
                self.set_permission_mode(mode, process_config).await?;
            }
            thought_level::THOUGHT_LEVEL_ID => {
                let requested = thought_level::parse_thought_level_request(&request)?;
                thought_level::apply_thought_level(
                    &self.built,
                    &self.thought_config(process_config),
                    requested,
                )?;
            }
            fast_mode::FAST_MODE_ID => {
                let enabled = fast_mode::parse_fast_mode_request(&request)?;
                fast_mode::apply_fast_mode(&self.built.session, &self.current_model(), enabled)?;
            }
            _ => {
                return Ok(SetSessionConfigOptionResponse::new(
                    self.set_model_option(&request, process_config).await?,
                ));
            }
        }
        Ok(SetSessionConfigOptionResponse::new(
            self.config_options(process_config),
        ))
    }

    pub(super) fn cancel_handle(&self) -> Arc<PromptGate> {
        Arc::clone(&self.prompt_gate)
    }

    pub(super) fn replaced_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.replaced)
    }

    /// The commands this session runs, as an `available_commands_update`.
    pub(super) fn available_commands(&self, process_config: &Config) -> SessionNotification {
        let home = crate::paths::home_dir();
        let commands =
            commands::available_commands(&self.command_scope(home.as_deref(), process_config));
        SessionNotification::new(
            self.acp_session_id.clone(),
            SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(commands)),
        )
    }

    fn command_scope<'a>(
        &'a self,
        home: Option<&'a std::path::Path>,
        process_config: &'a Config,
    ) -> CommandScope<'a> {
        CommandScope {
            cwd: self.stored.cwd(),
            home,
            tools: &self.built.tools,
            config: process_config,
            permission_mode: self.permission.mode,
        }
    }

    pub(super) fn config_options(&self, process_config: &Config) -> Vec<SessionConfigOption> {
        advertised_config_options(
            &self.current_model(),
            process_config,
            CurrentSettings {
                permission_mode: self.permission.mode,
                reasoning: self.built.session.reasoning_level(),
                service_tier: self.built.session.service_tier(),
            },
        )
    }

    /// Resolves and applies `session/set_config_option` on this host. The caller
    /// holds the host slot lock so no prompt is in flight.
    pub(super) async fn set_model_option(
        &mut self,
        request: &SetSessionConfigOptionRequest,
        process_config: &Config,
    ) -> Result<Vec<SessionConfigOption>, AcpError> {
        let current = self.current_model();
        let available_auths = available_auth_modes(&AppCredentialStore);
        let selection = config_options::resolve_model_value(request, &current, &available_auths)?;
        if model_reference(&selection.provider, &selection.model)
            == model_reference(&current.provider, &current.model)
            && selection.auth == current.auth
        {
            return Ok(self.config_options(process_config));
        }
        let reasoning = resolve_switch_reasoning(&selection, self.built.session.reasoning_level())?;
        let mut config = process_config.clone();
        config.provider = selection.provider.clone();
        config.model = selection.model.clone();
        config.auth = selection.auth.clone();
        config.reasoning = reasoning;
        let provider =
            build_provider_from_config_ensuring_catalog(&config, Arc::new(AppCredentialStore))
                .await
                .map_err(host_apply_error)?;
        self.switch_provider(provider, &config)?;
        Ok(self.config_options(process_config))
    }

    /// Installs an already-resolved provider while the ACP host slot is held.
    /// Kept separate from credential discovery so the session transaction can
    /// be exercised with a scripted provider.
    fn switch_provider(
        &mut self,
        provider: Arc<dyn rho_sdk::provider::ModelProvider>,
        config: &Config,
    ) -> Result<(), AcpError> {
        let current = self.current_model();
        let previous_context_window = model_context_window(&current.provider, &current.model);
        let context_window = model_context_window(&config.provider, &config.model);
        let prepared_prompt = self
            .built
            .prompt_template
            .as_ref()
            .map(|template| {
                template.build(&crate::model_identity::PromptModel::from_sdk_identity(
                    &provider.identity(),
                ))
            })
            .transpose()
            .map_err(host_apply_error)?;
        // HandoffReport could ride along in `_meta` later; ACP has no place for it yet.
        let _handoff = apply_conversation_switch(
            ConversationSwitch {
                prepared_prompt: prepared_prompt.as_ref(),
                session: &self.built.session,
                tools: &self.built.tools,
                previous_provider: Arc::clone(&self.built.provider),
                new_provider: Arc::clone(&provider),
                new_reasoning: config.reasoning,
                auth: &config.auth,
                compaction: CompactionConfig::from(config),
                context_window,
                previous_context_window,
                usage_recording: self.built.runtime.usage_recording(),
                diagnostics: self.built.diagnostics.clone(),
            },
            SwitchNotice::SessionMessage,
        )
        .map_err(host_apply_error)?;
        self.built.provider = provider;
        if let Some(prompt) = prepared_prompt {
            self.built.prompt.adopt(
                crate::app::active_prompt::ActivePrompt::from_prepared(prompt),
                &self.built.diagnostics,
                self.built.tools.advisor(),
            );
        }
        self.auth = config.auth.clone();
        Ok(())
    }

    pub(super) async fn shutdown(self) {
        self.prompt_gate.cancel();
        let Self { built, herdr, .. } = self;
        built.teardown().await;
        let _ = herdr.release().await;
    }

    fn current_model(&self) -> CurrentModel {
        let snapshot = self.built.session.snapshot();
        let identity = snapshot.provider();
        CurrentModel {
            provider: identity.provider.clone(),
            model: identity.model.clone(),
            auth: self.auth.clone(),
        }
    }

    fn thought_config(&self, process_config: &Config) -> Config {
        model_scoped_config(&self.current_model(), process_config)
    }

    fn from_built(
        acp_session_id: SessionId,
        built: BuiltSession,
        stored: StoredSession,
        added_dirs: crate::added_dirs::AddedDirs,
        permission: SessionPermission,
        auth: String,
        herdr: HerdrReporter,
    ) -> Self {
        built.tools.bind_recall(Some(&stored));
        Self {
            acp_session_id,
            built,
            stored,
            added_dirs,
            permission,
            auth,
            prompt_gate: Arc::new(PromptGate::new()),
            completed_runs: 0,
            permission_placeholders: AtomicU64::new(0),
            replaced: Arc::new(AtomicBool::new(false)),
            unsaved_compaction: false,
            herdr,
            sent_plan: Vec::new(),
            load_notice: None,
        }
    }

    /// A model turn on `model`, saved for display as `display`, the prompt
    /// as the user sent it. `call` runs before the first model request.
    async fn run_turn(
        &mut self,
        display: &UserInput,
        model: UserInput,
        call: Option<rho_sdk::model::ToolCall>,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let mut delegation = crate::app::headless_delegation::HeadlessDelegation::attach(
            &self.built.session,
            crate::app::headless_delegation::BackgroundSources::of(&self.built.tools),
            Some(&self.stored),
        )?;
        let result = crate::app::headless_delegation::HeadlessDelegation::drive(
            &mut delegation,
            self.drive_prompt(display, model, call, client),
        )
        .await;
        if let Some(manager) = self.built.tools.subagents() {
            manager.shutdown().await;
            // Cancellation, errors, and exhausted step budgets end this
            // prompt's automatic delivery. Keep snapshots for explicit status
            // queries, but do not wait on or inject leftover children into the
            // next prompt, including runs still alive after shutdown timed out.
            manager.end_automatic_delivery(self.built.session.id().as_str());
        }
        result
    }

    async fn drive_prompt(
        &mut self,
        display: &UserInput,
        model: UserInput,
        call: Option<rho_sdk::model::ToolCall>,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        if let Some(notice) = self.load_notice.take() {
            let notice = EventMapper::agent_notice(&self.acp_session_id, &notice);
            if let Err(error) = send_notification(client, notice).await {
                tracing::debug!(%error, "could not send ACP load notice");
            }
        }
        let checkpoint = self.install_turn_checkpoint(display)?;
        let result = self.drive_run(&checkpoint, model, call, client).await;
        // The run is over, so the session is idle again.
        if let Err(error) = self.built.session.set_checkpoint_store(None) {
            tracing::warn!(%error, "could not remove ACP turn checkpoint store");
        }
        result
    }

    async fn drive_run(
        &mut self,
        checkpoint: &TurnCheckpoint,
        model: UserInput,
        call: Option<rho_sdk::model::ToolCall>,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let started = match call {
            Some(call) => self.built.session.start_with_tool_call(model, call).await,
            None => self.built.session.start(model).await,
        };
        let mut run = match started {
            Ok(run) => run,
            Err(error) => {
                self.dispatch_failed(&error.to_string());
                return Err(error.into());
            }
        };
        self.prompt_gate.activate(run.cancellation_handle());
        let mut approval_receiver = self.built.approval_receiver.take();
        let mut mapper = EventMapper::new();
        let mut signals = self.signals();
        signals.send_plan_change(&self.acp_session_id, client).await;
        let pump_result = self
            .pump_run(
                &mut run,
                &mut mapper,
                &mut signals,
                client,
                approval_receiver.as_mut(),
            )
            .await;
        self.built.approval_receiver = approval_receiver;
        if let Err(error) = pump_result {
            run.cancel();
            let _ = run.outcome().await;
            self.dispatch_failed(&error.to_string());
            return Err(error);
        }
        let outcome = run.outcome().await;
        // The committed turn is the settled context; in-run updates were estimates.
        signals.send_usage(&self.acp_session_id, client).await;
        self.sent_plan = signals.into_sent_plan();
        match outcome {
            Ok(outcome) => {
                if let Err(error) = self.persist_turn(checkpoint, Some(outcome.text())) {
                    self.dispatch_failed(&error.to_string());
                    return Err(error);
                }
                self.completed_runs = self.completed_runs.saturating_add(1);
                self.built
                    .runtime
                    .hooks()
                    .session_completed(self.built.session.id(), self.completed_runs);
                Ok(PromptResponse::new(EventMapper::map_stop(&outcome)))
            }
            Err(rho_sdk::Error::Cancelled) => {
                if let Err(error) = self.persist_turn(checkpoint, None) {
                    self.dispatch_failed(&error.to_string());
                    return Err(error);
                }
                Ok(PromptResponse::new(
                    agent_client_protocol::schema::v1::StopReason::Cancelled,
                ))
            }
            Err(error) => {
                self.dispatch_failed(&error.to_string());
                Err(error.into())
            }
        }
    }

    async fn pump_run(
        &mut self,
        run: &mut rho_sdk::Run,
        mapper: &mut EventMapper,
        signals: &mut SessionSignals,
        client: &dyn AcpClientPort,
        approval_receiver: Option<&mut ApprovalRequestReceiver>,
    ) -> anyhow::Result<()> {
        let approvals_open = approval_receiver.is_some();
        pump_sources(PumpSources {
            session_id: &self.acp_session_id,
            cancel: run.cancellation_handle(),
            mapper,
            signals: Some(signals),
            client,
            placeholders: &self.permission_placeholders,
            events: &mut EventSource::Run(run),
            approvals: &mut ApprovalSource::Receiver(approval_receiver),
            approvals_open,
        })
        .await
    }

    /// Signal tracking for the next prompt, starting from the checklist the
    /// client last saw. An early return before the prompt hands it back only
    /// costs a resend of the current list.
    fn signals(&mut self) -> SessionSignals {
        let current = self.current_model();
        SessionSignals::new(
            self.built.session.clone(),
            self.built.tools.todo_state(),
            model_context_window(&current.provider, &current.model),
            std::mem::take(&mut self.sent_plan),
        )
    }

    fn dispatch_failed(&self, message: &str) {
        self.built.runtime.hooks().session_failed(
            self.built.session.id(),
            rho_sdk::hooks::HookSessionFailureKind::RunFailed,
            message,
        );
    }
}

/// Session settings the advertised options report as current values.
#[derive(Clone, Copy)]
struct CurrentSettings {
    permission_mode: PermissionMode,
    reasoning: rho_sdk::ReasoningLevel,
    service_tier: Option<rho_sdk::model::ServiceTier>,
}

/// Every option a session advertises, rebuilt after each change so a model
/// switch adds or drops the per-model `thought_level` and `fast_mode`. The
/// permission `mode` comes first, as the ACP spec's own example orders it.
fn advertised_config_options(
    current: &CurrentModel,
    process_config: &Config,
    settings: CurrentSettings,
) -> Vec<SessionConfigOption> {
    let available_auths = available_auth_modes(&AppCredentialStore);
    let mut options = vec![permission::mode_config_option(settings.permission_mode)];
    options.extend(config_options::model_config_options(
        current,
        &process_config.favorite_models,
        catalog::available_models_for_auths(&available_auths),
    ));
    options.extend(thought_level::config_options(
        &model_scoped_config(current, process_config),
        settings.reasoning,
    ));
    options.extend(fast_mode::config_options(current, settings.service_tier));
    options
}

fn model_scoped_config(current: &CurrentModel, process_config: &Config) -> Config {
    let mut config = process_config.clone();
    config.provider = current.provider.clone();
    config.model = current.model.clone();
    config.auth = current.auth.clone();
    config
}

fn model_context_window(provider: &str, model: &str) -> Option<u64> {
    models_dev::cached_model_metadata(provider, model)
        .and_then(|metadata| metadata.display_context_window())
}

fn resolve_switch_reasoning(
    selection: &ModelSelection,
    requested: rho_sdk::ReasoningLevel,
) -> Result<rho_sdk::ReasoningLevel, AcpError> {
    let capabilities =
        models_dev::current_reasoning_capabilities(&selection.provider, &selection.model);
    match resolve_model_switch_reasoning(
        &capabilities,
        requested,
        ReasoningRequestSource::PersistedOrDefault,
        crate::app::conversation_switch::ModelSwitchReasoningPolicy::PreserveExplicit,
    ) {
        Ok(resolved) => Ok(resolved.effective),
        Err(level) => Err(AcpError::invalid_params().data(format!(
            "reasoning level '{level}' is not supported for '{}'",
            model_reference(&selection.provider, &selection.model)
        ))),
    }
}

fn host_apply_error(error: impl ToString) -> AcpError {
    AcpError::internal_error().data(error.to_string())
}

fn split_assembly(
    assembly: crate::app::session_assembly::SessionAssembly,
    mode: crate::permission::PermissionMode,
) -> (BuiltSession, SessionPermission) {
    let crate::app::session_assembly::SessionAssembly {
        built,
        workspace_root,
        session_writes,
    } = assembly;
    (
        built,
        SessionPermission {
            mode,
            writes: session_writes,
            workspace_root,
        },
    )
}

async fn replay_display_history(
    session_id: &SessionId,
    messages: &[Message],
    client: &dyn AcpClientPort,
) -> anyhow::Result<()> {
    for notification in EventMapper::replay_history(session_id, messages) {
        send_notification(client, notification).await?;
    }
    Ok(())
}

async fn send_notification(
    client: &dyn AcpClientPort,
    notification: SessionNotification,
) -> anyhow::Result<()> {
    client
        .send_session_notification(notification)
        .await
        .map_err(|error| anyhow::Error::msg(error.to_string()))
}

enum EventSource<'a> {
    Run(&'a mut rho_sdk::Run),
    #[cfg(test)]
    Channel(&'a mut tokio::sync::mpsc::UnboundedReceiver<RunEvent>),
}

impl EventSource<'_> {
    async fn next(&mut self) -> Option<RunEvent> {
        match self {
            Self::Run(run) => run.next_event().await,
            #[cfg(test)]
            Self::Channel(events) => events.recv().await,
        }
    }

    fn cancel_run(&mut self) {
        match self {
            Self::Run(run) => run.cancel(),
            #[cfg(test)]
            Self::Channel(_) => {}
        }
    }
}

enum ApprovalSource<'a> {
    Receiver(Option<&'a mut ApprovalRequestReceiver>),
    #[cfg(test)]
    Channel(&'a mut tokio::sync::mpsc::UnboundedReceiver<PendingApproval>),
}

impl ApprovalSource<'_> {
    async fn next(&mut self) -> Option<PendingApproval> {
        match self {
            Self::Receiver(None) => std::future::pending().await,
            Self::Receiver(Some(receiver)) => receiver.recv().await,
            #[cfg(test)]
            Self::Channel(approvals) => approvals.recv().await,
        }
    }
}

struct PumpSources<'a> {
    session_id: &'a SessionId,
    cancel: CancellationToken,
    mapper: &'a mut EventMapper,
    /// Session state mirrored alongside events; tests that only exercise the
    /// event and approval plumbing pass `None`.
    signals: Option<&'a mut SessionSignals>,
    client: &'a dyn AcpClientPort,
    placeholders: &'a AtomicU64,
    events: &'a mut EventSource<'a>,
    approvals: &'a mut ApprovalSource<'a>,
    approvals_open: bool,
}

async fn pump_sources(sources: PumpSources<'_>) -> anyhow::Result<()> {
    let PumpSources {
        session_id,
        cancel,
        mapper,
        mut signals,
        client,
        placeholders,
        events,
        approvals,
        mut approvals_open,
    } = sources;
    let mut run_cancelled = cancel.is_cancelled();
    let mut inflight: FuturesUnordered<Pin<Box<dyn Future<Output = ()> + Send + '_>>> =
        FuturesUnordered::new();
    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled(), if !run_cancelled => {
                events.cancel_run();
                run_cancelled = true;
            }

            event = events.next() => {
                let Some(event) = event else {
                    while inflight.next().await.is_some() {}
                    return Ok(());
                };
                if let Some(notification) = mapper.map_event(session_id, &event) {
                    send_notification(client, notification).await?;
                }
                let signal = signals.as_deref_mut().and_then(|signals| signals.observe(&event));
                if let Some(update) = signal {
                    let notification = SessionNotification::new(session_id.clone(), update);
                    send_notification(client, notification).await?;
                }
            }

            pending = approvals.next(), if approvals_open => {
                let Some(pending) = pending else {
                    approvals_open = false;
                    continue;
                };
                inflight.push(Box::pin(answer_approval(
                    session_id.clone(),
                    pending,
                    client,
                    cancel.clone(),
                    placeholders,
                )));
            }

            Some(()) = inflight.next(), if !inflight.is_empty() => {}
        }
    }
}

async fn answer_approval(
    session_id: SessionId,
    mut pending: PendingApproval,
    client: &dyn AcpClientPort,
    cancel: CancellationToken,
    placeholders: &AtomicU64,
) {
    let placeholder = permission::next_placeholder_tool_call_id(placeholders);
    let request = permission::permission_request(&session_id, pending.request(), &placeholder);
    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => RequestPermissionOutcome::Cancelled,
        response = client.request_permission(request) => match response {
            Ok(response) => response.outcome,
            Err(_) => RequestPermissionOutcome::Cancelled,
        },
    };
    let _ = pending.respond(permission::decision_for(&outcome));
}

#[cfg(test)]
#[path = "session_host_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "session_host_model_prompt_tests.rs"]
mod model_prompt_tests;
