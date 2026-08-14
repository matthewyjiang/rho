use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, RequestPermissionOutcome, SessionId,
};
use rho_sdk::{
    model::Message, ApprovalRequestReceiver, CancellationToken, PendingApproval, SessionOptions,
    UserInput,
};

use super::{events::EventMapper, permission, AcpClientPort};
use crate::{
    herdr::{HerdrReporter, HerdrState},
    session::Session as StoredSession,
    tools::sdk_registry::AppToolSet,
};

#[path = "session_host_build.rs"]
mod build;
#[path = "session_host_convert.rs"]
mod convert;

use build::{build_session, mode_state, prompt_cache_key, teardown_session, BuiltSession};
use convert::{user_input_from_prompt, validate_session_cwd, workspace_cwd};

pub(super) struct SessionBuildContext<'a> {
    pub config: &'a crate::config::Config,
    pub config_path: &'a std::path::Path,
    pub process_cwd: &'a std::path::Path,
    pub no_system_prompt: bool,
    pub no_tools: bool,
    pub no_subagents: bool,
    pub agent: &'a crate::app::agent_binding::BoundAgent,
    pub diagnostics: &'a crate::diagnostics::RuntimeDiagnostics,
    pub herdr: &'a crate::herdr::HerdrReporter,
}

pub(super) struct SessionHost {
    acp_session_id: SessionId,
    runtime: rho_sdk::Rho,
    session: rho_sdk::Session,
    stored: StoredSession,
    tools: AppToolSet,
    hooks: Option<crate::hooks::HookPipeline>,
    approval_receiver: Option<ApprovalRequestReceiver>,
    mapper: EventMapper,
    prompt_gate: Arc<PromptGate>,
    completed_runs: u64,
    herdr: HerdrReporter,
}

/// One-prompt-at-a-time slot. `cancel` is a no-op while idle.
pub(super) struct PromptGate {
    slot: Mutex<PromptGateState>,
}

#[derive(Debug)]
enum PromptGateState {
    Idle,
    Starting { cancelled: bool },
    Active(CancellationToken),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ActivePromptError;

impl std::fmt::Display for ActivePromptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ACP session already has an active prompt")
    }
}

impl std::error::Error for ActivePromptError {}

impl PromptGate {
    pub(super) fn new() -> Self {
        Self {
            slot: Mutex::new(PromptGateState::Idle),
        }
    }

    pub(super) fn try_begin(&self) -> Result<(), ActivePromptError> {
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match *slot {
            PromptGateState::Idle => {
                *slot = PromptGateState::Starting { cancelled: false };
                Ok(())
            }
            PromptGateState::Starting { .. } | PromptGateState::Active(_) => Err(ActivePromptError),
        }
    }

    fn activate(&self, token: CancellationToken) {
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cancelled = matches!(*slot, PromptGateState::Starting { cancelled: true });
        if cancelled {
            token.cancel();
        }
        *slot = PromptGateState::Active(token);
    }

    pub(super) fn finish(&self) {
        *self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = PromptGateState::Idle;
    }

    pub(super) fn cancel(&self) {
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &mut *slot {
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
        ctx: SessionBuildContext<'_>,
        request: NewSessionRequest,
    ) -> anyhow::Result<(Self, NewSessionResponse)> {
        let cwd = validate_session_cwd(&request.cwd)?;
        // Host-supplied MCP servers are ignored. Rho loads MCP from workspace
        // and config through assemble_tools_and_prompt, not request.mcpServers.
        let _ = request.mcp_servers;
        let sdk_id = rho_sdk::SessionId::new();
        let cache_key = prompt_cache_key(sdk_id.as_str());
        let built = build_session(&ctx, cwd, |_| {
            Ok(SessionOptions::new()
                .id(sdk_id.clone())
                .prompt_cache_key(cache_key.clone()))
        })
        .await?;
        let stored = match StoredSession::create_with_id(
            cwd,
            built.session.id().as_str(),
            ctx.agent.id().as_str(),
            &ctx.agent.fingerprint().to_string(),
        ) {
            Ok(stored) => stored,
            Err(error) => {
                let BuiltSession {
                    runtime,
                    session,
                    tools,
                    hooks,
                    ..
                } = built;
                teardown_session(runtime, session, hooks, tools).await;
                return Err(error);
            }
        };
        let acp_session_id = SessionId::new(built.session.id().as_str());
        let response = NewSessionResponse::new(acp_session_id.clone())
            .modes(mode_state(ctx.config.permission_mode));
        Ok((
            Self::from_built(acp_session_id, built, stored, ctx.herdr.clone()),
            response,
        ))
    }

    pub(super) async fn load(
        ctx: SessionBuildContext<'_>,
        request: LoadSessionRequest,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<(Self, LoadSessionResponse)> {
        let cwd = workspace_cwd(&request.cwd, ctx.process_cwd);
        if !request.cwd.as_os_str().is_empty() {
            validate_session_cwd(cwd)?;
        }
        let _ = request.mcp_servers;
        let (stored, histories) =
            StoredSession::open_by_id_with_histories(cwd, request.session_id.0.as_ref())?;
        let built = build_session(&ctx, cwd, |provider| {
            let snapshot =
                stored.snapshot_for_resume(provider.identity(), prompt_cache_key(stored.id()))?;
            Ok(SessionOptions::from_snapshot(snapshot))
        })
        .await?;
        let replay_result =
            replay_display_history(&request.session_id, &histories.display, client).await;
        if let Err(error) = replay_result {
            let BuiltSession {
                runtime,
                session,
                tools,
                hooks,
                ..
            } = built;
            teardown_session(runtime, session, hooks, tools).await;
            return Err(error);
        }
        let response = LoadSessionResponse::new().modes(mode_state(ctx.config.permission_mode));
        Ok((
            Self::from_built(request.session_id, built, stored, ctx.herdr.clone()),
            response,
        ))
    }

    pub(super) async fn prompt(
        &mut self,
        request: PromptRequest,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let input = user_input_from_prompt(&request.prompt)?;
        self.prompt_gate.try_begin()?;
        let _guard = PromptGuard(Arc::clone(&self.prompt_gate));
        self.mapper = EventMapper::new();
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

    pub(super) fn cancel_handle(&self) -> Arc<PromptGate> {
        Arc::clone(&self.prompt_gate)
    }

    pub(super) async fn shutdown(self) {
        self.prompt_gate.cancel();
        let Self {
            runtime,
            session,
            tools,
            hooks,
            herdr,
            ..
        } = self;
        teardown_session(runtime, session, hooks, tools).await;
        herdr.release().await;
    }

    fn from_built(
        acp_session_id: SessionId,
        built: BuiltSession,
        stored: StoredSession,
        herdr: HerdrReporter,
    ) -> Self {
        Self {
            acp_session_id,
            runtime: built.runtime,
            session: built.session,
            stored,
            tools: built.tools,
            hooks: built.hooks,
            approval_receiver: built.approval_receiver,
            mapper: EventMapper::new(),
            prompt_gate: Arc::new(PromptGate::new()),
            completed_runs: 0,
            herdr,
        }
    }

    async fn drive_prompt(
        &mut self,
        input: UserInput,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let mut run = match self.session.start(input.clone()).await {
            Ok(run) => run,
            Err(error) => {
                self.dispatch_failed(&error.to_string());
                return Err(error.into());
            }
        };
        self.prompt_gate.activate(run.cancellation_handle());
        let mut approval_receiver = self.approval_receiver.take();
        let pump_result = self
            .pump_run(&mut run, client, approval_receiver.as_mut())
            .await;
        self.approval_receiver = approval_receiver;
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
                self.runtime
                    .hooks()
                    .session_completed(self.session.id(), self.completed_runs);
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
        client: &dyn AcpClientPort,
        approval_receiver: Option<&mut ApprovalRequestReceiver>,
    ) -> anyhow::Result<()> {
        let cancel = run.cancellation_handle();
        let mut run_cancelled = cancel.is_cancelled();
        let mut approval_receiver = approval_receiver;
        let mut approvals_open = approval_receiver.is_some();
        loop {
            tokio::select! {
                biased;

                _ = cancel.cancelled(), if !run_cancelled => {
                    run.cancel();
                    run_cancelled = true;
                }

                event = run.next_event() => {
                    let Some(event) = event else {
                        return Ok(());
                    };
                    for notification in self.mapper.map_event(&self.acp_session_id, &event) {
                        send_notification(client, notification).await?;
                    }
                }

                pending = recv_approval(approval_receiver.as_deref_mut()), if approvals_open => {
                    let Some(pending) = pending else {
                        approvals_open = false;
                        continue;
                    };
                    answer_approval(&self.acp_session_id, pending, client, &cancel).await;
                }
            }
        }
    }

    fn persist_turn(&self, input: &UserInput, assistant_text: Option<&str>) -> anyhow::Result<()> {
        let mut display_tail = vec![Message::User(input.blocks().to_vec())];
        if let Some(text) = assistant_text.filter(|text| !text.is_empty()) {
            display_tail.push(Message::assistant_text(text));
        }
        self.stored
            .save_snapshot(&self.session.snapshot(), &display_tail)
    }

    fn dispatch_failed(&self, message: &str) {
        self.runtime.hooks().session_failed(
            self.session.id(),
            rho_sdk::hooks::HookSessionFailureKind::RunFailed,
            message,
        );
    }
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
    notification: agent_client_protocol::schema::v1::SessionNotification,
) -> anyhow::Result<()> {
    client
        .send_session_notification(notification)
        .await
        .map_err(|error| anyhow::Error::msg(error.to_string()))
}

async fn recv_approval(receiver: Option<&mut ApprovalRequestReceiver>) -> Option<PendingApproval> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn answer_approval(
    session_id: &SessionId,
    mut pending: PendingApproval,
    client: &dyn AcpClientPort,
    cancel: &CancellationToken,
) {
    let request = permission::permission_request(session_id, pending.request());
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
