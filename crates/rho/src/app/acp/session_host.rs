use std::sync::{
    atomic::{AtomicBool, AtomicU64},
    Arc, Mutex,
};

use agent_client_protocol::{
    schema::v1::{
        AvailableCommandsUpdate, McpServer, PromptRequest, PromptResponse, SessionConfigOption,
        SessionId, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
        SetSessionConfigOptionResponse,
    },
    Error as AcpError,
};
use rho_providers::{
    credentials::available_auth_modes,
    model::{
        catalog::{self, ModelSelection},
        models_dev, ReasoningRequestSource,
    },
    provider::model_reference,
};
use rho_sdk::{ApprovalRequestReceiver, CancellationToken, UserInput};

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
        agent_binding::BoundAgent,
        conversation_switch::{
            apply_conversation_switch, resolve_model_switch_reasoning, ConversationSwitch,
            SwitchNotice,
        },
        headless_delegation::OfferedDisplays,
        interactive_runtime::startup::session_options_for_id,
        session_assembly::BuiltSession,
    },
    compaction::CompactionConfig,
    config::Config,
    credential_store::{build_provider_from_config_ensuring_catalog, AppCredentialStore},
    herdr::{HerdrReporter, HerdrSession, HerdrState},
    permission::PermissionMode,
    session::Session as StoredSession,
};

#[path = "session_host_agent.rs"]
mod agent;
#[path = "session_host_build.rs"]
mod build;
#[path = "session_host_compact.rs"]
mod compact;
#[path = "session_host_convert.rs"]
mod convert;
#[path = "session_host_mode.rs"]
mod mode;
#[path = "session_host_open.rs"]
mod open;
#[path = "session_host_persist.rs"]
mod persist;
#[path = "session_host_pump.rs"]
mod pump;
#[path = "session_host_title.rs"]
mod title;
#[path = "session_host_workflow.rs"]
mod workflow;

use build::{build_session, session_config};
use convert::user_input_from_prompt;
pub(super) use convert::RejectedDirectory;
use mode::SessionPermission;
use persist::{HostStorage, TurnCheckpoint};
use pump::{pump_sources, AppliedDisplays, ApprovalSource, EventSource, PumpSources};

pub(super) struct SessionHost {
    acp_session_id: SessionId,
    built: BuiltSession,
    storage: HostStorage,
    agent: BoundAgent,
    /// Retained so an agent rebind reconnects the session's host-supplied tools.
    host_mcp_servers: Vec<McpServer>,
    /// Launch `--add-dir` values plus the host's `additionalDirectories`.
    /// Saved with every snapshot so a TUI resume restores them; a later
    /// `session/load` takes only the roots its request names.
    added_dirs: crate::added_dirs::AddedDirs,
    permission: SessionPermission,
    auth: String,
    prompt_gate: Arc<PromptGate>,
    completed_runs: u64,
    /// Whether this host asked the title agent to name the session.
    title_requested: bool,
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
    /// Child notice binding kept between prompts, since children outlive them.
    held_notices: Option<crate::app::headless_delegation::HeldNotices>,
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
    /// Runs one `session/prompt`: a slash command or a model turn. A
    /// command that cannot run fails with [`commands::CommandRejected`].
    /// `startup` supplies process defaults and the available root capabilities
    /// when a fresh session binds an agent definition.
    pub(super) async fn prompt(
        &mut self,
        request: PromptRequest,
        client: &dyn AcpClientPort,
        startup: &AcpStartup,
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
        let scope = self.command_scope(home.as_deref(), &startup.config);
        // Expansion can wait on an MCP server, so a cancel drops it.
        let action = tokio::select! {
            biased;
            () = starting.cancelled() => None,
            action = commands::resolve(input.clone(), &scope) => Some(action),
        };
        // The first model turn this host completes names an untitled session.
        let names_session = matches!(
            &action,
            Some(Ok(
                PromptAction::Turn { .. } | PromptAction::RunAsAgent { .. }
            ))
        ) && !self.title_requested;
        let result = match action {
            None => Ok(PromptResponse::new(
                agent_client_protocol::schema::v1::StopReason::Cancelled,
            )),
            Some(Ok(PromptAction::Turn {
                input: model,
                calls,
            })) => self.run_turn(&input, model, calls, client).await,
            Some(Ok(PromptAction::Compact { instructions })) => {
                self.compact(instructions, client).await
            }
            Some(Ok(PromptAction::StartWorkflow(start))) => {
                self.start_workflow(&input, start, &startup.config, &startup.config_path, client)
                    .await
            }
            Some(Ok(PromptAction::RunAsAgent {
                agent_id,
                input: task,
            })) => {
                self.run_as_agent(&input, &agent_id, task, startup, client)
                    .await
            }
            Some(Err(rejected)) => Err(rejected.into()),
        };
        let completed = result.as_ref().is_ok_and(|response| {
            response.stop_reason == agent_client_protocol::schema::v1::StopReason::EndTurn
        });
        if names_session && completed {
            self.spawn_title(&startup.config, &input);
        }
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
            cwd: self.storage.cwd(),
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
        storage: HostStorage,
        added_dirs: crate::added_dirs::AddedDirs,
        permission: SessionPermission,
        startup: &AcpStartup,
        host_mcp_servers: Vec<McpServer>,
    ) -> Self {
        built.tools.bind_recall(storage.file());
        Self {
            acp_session_id,
            built,
            storage,
            agent: startup.agent.clone(),
            host_mcp_servers,
            added_dirs,
            permission,
            auth: startup.config.auth.clone(),
            prompt_gate: Arc::new(PromptGate::new()),
            completed_runs: 0,
            title_requested: false,
            permission_placeholders: AtomicU64::new(0),
            replaced: Arc::new(AtomicBool::new(false)),
            unsaved_compaction: false,
            herdr: startup.herdr.clone(),
            sent_plan: Vec::new(),
            load_notice: None,
            held_notices: None,
        }
    }

    /// A model turn on `model`, saved for display as `display`, the prompt
    /// as the user sent it. `calls` run as one host batch before the first
    /// model request.
    async fn run_turn(
        &mut self,
        display: &UserInput,
        model: UserInput,
        calls: Vec<rho_sdk::model::ToolCall>,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let stored = self.stored()?.clone();
        let mut delegation = crate::app::headless_delegation::HeadlessDelegation::attach(
            &self.built.session,
            crate::app::headless_delegation::BackgroundSources::of(&self.built.tools),
            Some(&stored),
            &mut self.held_notices,
        )?;
        let displays = delegation.offered_displays();
        // Children outlive a cancelled, failed, or step-limited prompt, as they
        // do across TUI turns: T3 Code cancels the open prompt to send each
        // mid-turn message. The next prompt waits for and delivers them and
        // their undelivered notices; only closing or replacing the session
        // stops them.
        let result = crate::app::headless_delegation::HeadlessDelegation::drive(
            &mut delegation,
            self.drive_prompt(display, model, calls, displays, client),
        )
        .await;
        self.held_notices = delegation.hold_notices();
        result
    }

    async fn drive_prompt(
        &mut self,
        display: &UserInput,
        model: UserInput,
        calls: Vec<rho_sdk::model::ToolCall>,
        displays: OfferedDisplays,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        if let Some(notice) = self.load_notice.take() {
            let notice = EventMapper::agent_notice(&self.acp_session_id, &notice);
            if let Err(error) = send_notification(client, notice).await {
                tracing::debug!(%error, "could not send ACP load notice");
            }
        }
        let checkpoint = self.install_turn_checkpoint(display, &model)?;
        let result = self
            .drive_run(&checkpoint, model, calls, displays, client)
            .await;
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
        calls: Vec<rho_sdk::model::ToolCall>,
        displays: OfferedDisplays,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let started = if calls.is_empty() {
            self.built.session.start(model).await
        } else {
            self.built.session.start_with_tool_calls(model, calls).await
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
                AppliedDisplays {
                    offered: displays,
                    checkpoint,
                },
            )
            .await;
        self.built.approval_receiver = approval_receiver;
        if let Err(error) = pump_result {
            run.cancel();
            let _ = run.outcome().await;
            self.persist_failed_turn(checkpoint);
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
                self.persist_failed_turn(checkpoint);
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
        displays: AppliedDisplays<'_>,
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
            displays: Some(displays),
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

async fn send_notification(
    client: &dyn AcpClientPort,
    notification: SessionNotification,
) -> anyhow::Result<()> {
    client
        .send_session_notification(notification)
        .await
        .map_err(|error| anyhow::Error::msg(error.to_string()))
}

#[cfg(test)]
#[path = "session_host_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "session_host_model_prompt_tests.rs"]
mod model_prompt_tests;

#[cfg(test)]
#[path = "session_host_delegation_tests.rs"]
mod delegation_tests;
