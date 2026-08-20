use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use agent_client_protocol::{
    schema::v1::{
        LoadSessionRequest, LoadSessionResponse, McpServer, NewSessionRequest, NewSessionResponse,
        PromptRequest, PromptResponse, RequestPermissionOutcome, SessionConfigOption, SessionId,
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
    config_options::{self, CurrentModel},
    events::EventMapper,
    permission, thought_level, AcpClientPort, AcpStartup,
};
use crate::{
    app::{
        conversation_switch::{
            apply_conversation_switch, resolve_model_switch_reasoning, ConversationSwitch,
            SwitchNotice,
        },
        interactive_runtime::startup::prompt_cache_key,
        session_assembly::BuiltSession,
    },
    compaction::CompactionConfig,
    config::Config,
    credential_store::{build_provider_from_config_ensuring_catalog, AppCredentialStore},
    herdr::{HerdrReporter, HerdrState},
    session::Session as StoredSession,
};

#[path = "session_host_build.rs"]
mod build;
#[path = "session_host_convert.rs"]
mod convert;

use build::{build_session, mode_state};
use convert::{user_input_from_prompt, validate_session_cwd, workspace_cwd};

pub(super) struct SessionHost {
    acp_session_id: SessionId,
    built: BuiltSession,
    stored: StoredSession,
    auth: String,
    prompt_gate: Arc<PromptGate>,
    completed_runs: u64,
    permission_placeholders: AtomicU64,
    replaced: Arc<AtomicBool>,
    herdr: HerdrReporter,
}

/// Carries cancellation for the session's current prompt. The agent already
/// serializes prompts with `try_lock` on the host slot, so this gate only has
/// to cover the window between prompt entry and the run's token existing: a
/// cancel that lands while `Starting` is replayed onto the token as soon as
/// `activate` receives it. `cancel` is a no-op while idle.
pub(super) struct PromptGate {
    slot: Mutex<PromptGateState>,
}

#[derive(Debug)]
enum PromptGateState {
    Idle,
    Starting { cancelled: bool },
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
    /// arrived for the previous one.
    pub(super) fn begin(&self) {
        *self.slot() = PromptGateState::Starting { cancelled: false };
    }

    fn activate(&self, token: CancellationToken) {
        let mut slot = self.slot();
        let cancelled = matches!(*slot, PromptGateState::Starting { cancelled: true });
        if cancelled {
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
            PromptGateState::Starting { cancelled } => *cancelled = true,
            PromptGateState::Active(token) => token.cancel(),
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
        ignore_host_mcp_servers(&request.mcp_servers);
        let sdk_id = rho_sdk::SessionId::new();
        let cache_key = prompt_cache_key(sdk_id.as_str());
        let built = build_session(startup, cwd, |_| {
            Ok(SessionOptions::new()
                .id(sdk_id.clone())
                .prompt_cache_key(cache_key.clone()))
        })
        .await?;
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
            startup.config.auth.clone(),
            startup.herdr.clone(),
        );
        let response = NewSessionResponse::new(acp_session_id)
            .modes(mode_state(startup.config.permission_mode))
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
        ignore_host_mcp_servers(&request.mcp_servers);
        let (stored, histories) =
            StoredSession::open_by_id_with_histories(cwd, request.session_id.0.as_ref())?;
        let built = build_session(startup, cwd, |provider| {
            let snapshot =
                stored.snapshot_for_resume(provider.identity(), prompt_cache_key(stored.id()))?;
            Ok(SessionOptions::from_snapshot(snapshot))
        })
        .await?;
        let replay_result =
            replay_display_history(&request.session_id, &histories.display, client).await;
        if let Err(error) = replay_result {
            built.teardown().await;
            return Err(error);
        }
        let host = Self::from_built(
            request.session_id,
            built,
            stored,
            startup.config.auth.clone(),
            startup.herdr.clone(),
        );
        let response = LoadSessionResponse::new()
            .modes(mode_state(startup.config.permission_mode))
            .config_options(host.config_options(&startup.config));
        Ok((host, response))
    }

    pub(super) async fn prompt(
        &mut self,
        request: PromptRequest,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let input = user_input_from_prompt(&request.prompt)?;
        self.prompt_gate.begin();
        let _guard = PromptGuard(Arc::clone(&self.prompt_gate));
        self.herdr
            .report_state(
                HerdrState::Working,
                None,
                Some(self.acp_session_id.0.as_ref()),
            )
            .await;
        let result = self.drive_prompt(input, client).await;
        self.herdr
            .report_state(HerdrState::Idle, None, Some(self.acp_session_id.0.as_ref()))
            .await;
        result
    }

    pub(super) async fn set_config_option(
        &mut self,
        request: SetSessionConfigOptionRequest,
        process_config: &Config,
    ) -> Result<SetSessionConfigOptionResponse, AcpError> {
        if request.config_id.0.as_ref() == thought_level::THOUGHT_LEVEL_ID {
            let requested = thought_level::parse_thought_level_request(&request)?;
            thought_level::apply_thought_level(
                &self.built,
                &self.thought_config(process_config),
                requested,
            )?;
            return Ok(SetSessionConfigOptionResponse::new(
                self.config_options(process_config),
            ));
        }
        Ok(SetSessionConfigOptionResponse::new(
            self.set_model_option(&request, process_config).await?,
        ))
    }

    pub(super) fn cancel_handle(&self) -> Arc<PromptGate> {
        Arc::clone(&self.prompt_gate)
    }

    pub(super) fn replaced_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.replaced)
    }

    pub(super) fn config_options(&self, process_config: &Config) -> Vec<SessionConfigOption> {
        let available_auths = available_auth_modes(&AppCredentialStore);
        let mut options = config_options::model_config_options(
            &self.current_model(),
            &process_config.favorite_models,
            catalog::available_models_for_auths(&available_auths),
        );
        options.extend(thought_level::config_options(
            &self.thought_config(process_config),
            self.built.session.reasoning_level(),
        ));
        options
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
        let previous_context_window = model_context_window(&current.provider, &current.model);
        let context_window = model_context_window(&selection.provider, &selection.model);
        let provider =
            build_provider_from_config_ensuring_catalog(&config, Arc::new(AppCredentialStore))
                .await
                .map_err(host_apply_error)?;
        // HandoffReport could ride along in `_meta` later; ACP has no place for it yet.
        let _handoff = apply_conversation_switch(
            ConversationSwitch {
                session: &self.built.session,
                tools: &self.built.tools,
                previous_provider: Arc::clone(&self.built.provider),
                new_provider: Arc::clone(&provider),
                new_reasoning: reasoning,
                auth: &selection.auth,
                compaction: CompactionConfig::from(process_config),
                context_window,
                previous_context_window,
                usage_recording: self.built.runtime.usage_recording(),
            },
            SwitchNotice::SessionMessage,
        )
        .map_err(host_apply_error)?;
        self.built.provider = provider;
        self.auth = selection.auth;
        Ok(self.config_options(process_config))
    }

    pub(super) async fn shutdown(self) {
        self.prompt_gate.cancel();
        let Self { built, herdr, .. } = self;
        built.teardown().await;
        herdr.release().await;
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
        let current = self.current_model();
        let mut config = process_config.clone();
        config.provider = current.provider;
        config.model = current.model;
        config.auth = current.auth;
        config
    }

    fn from_built(
        acp_session_id: SessionId,
        built: BuiltSession,
        stored: StoredSession,
        auth: String,
        herdr: HerdrReporter,
    ) -> Self {
        Self {
            acp_session_id,
            built,
            stored,
            auth,
            prompt_gate: Arc::new(PromptGate::new()),
            completed_runs: 0,
            permission_placeholders: AtomicU64::new(0),
            replaced: Arc::new(AtomicBool::new(false)),
            herdr,
        }
    }

    async fn drive_prompt(
        &mut self,
        input: UserInput,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let mut run = match self.built.session.start(input.clone()).await {
            Ok(run) => run,
            Err(error) => {
                self.dispatch_failed(&error.to_string());
                return Err(error.into());
            }
        };
        self.prompt_gate.activate(run.cancellation_handle());
        let mut approval_receiver = self.built.approval_receiver.take();
        let mut mapper = EventMapper::new();
        let pump_result = self
            .pump_run(&mut run, &mut mapper, client, approval_receiver.as_mut())
            .await;
        self.built.approval_receiver = approval_receiver;
        if let Err(error) = pump_result {
            run.cancel();
            let _ = run.outcome().await;
            self.dispatch_failed(&error.to_string());
            return Err(error);
        }
        match run.outcome().await {
            Ok(outcome) => {
                if let Err(error) = self.persist_turn(&input, Some(outcome.text())) {
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
                if let Err(error) = self.persist_turn(&input, None) {
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
        client: &dyn AcpClientPort,
        approval_receiver: Option<&mut ApprovalRequestReceiver>,
    ) -> anyhow::Result<()> {
        let approvals_open = approval_receiver.is_some();
        pump_sources(PumpSources {
            session_id: &self.acp_session_id,
            cancel: run.cancellation_handle(),
            mapper,
            client,
            placeholders: &self.permission_placeholders,
            events: &mut EventSource::Run(run),
            approvals: &mut ApprovalSource::Receiver(approval_receiver),
            approvals_open,
        })
        .await
    }

    fn persist_turn(&self, input: &UserInput, assistant_text: Option<&str>) -> anyhow::Result<()> {
        if self.replaced.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut display_tail = vec![Message::User(input.blocks().to_vec())];
        if let Some(text) = assistant_text.filter(|text| !text.is_empty()) {
            display_tail.push(Message::assistant_text(text));
        }
        self.stored
            .save_snapshot(&self.built.session.snapshot(), &display_tail)
    }

    fn dispatch_failed(&self, message: &str) {
        self.built.runtime.hooks().session_failed(
            self.built.session.id(),
            rho_sdk::hooks::HookSessionFailureKind::RunFailed,
            message,
        );
    }
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

/// Host-supplied MCP servers are ignored. Rho loads MCP from the workspace and
/// config through `assemble_tools_and_prompt`, not from `request.mcpServers`.
fn ignore_host_mcp_servers(_servers: &[McpServer]) {}

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
    notification: agent_client_protocol::schema::v1::SessionNotification,
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
