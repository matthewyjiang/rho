use std::{collections::HashMap, sync::Arc};

use agent_client_protocol::{
    schema::v1::{
        AgentCapabilities, AuthenticateRequest, CancelNotification, InitializeRequest,
        InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
        NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse, SessionId,
        SetSessionModeRequest,
    },
    Error as AcpError, ErrorCode,
};

use super::{
    session_host::{PromptGate, SessionBuildContext, SessionHost},
    AcpClientPort, AcpStartup,
};

/// Session entry that keeps a cancel handle even while `prompt` holds the host.
struct LiveSession {
    host: Option<SessionHost>,
    cancel: std::sync::Arc<PromptGate>,
}

/// In-process ACP agent. Session hosts live in a mutex map so request
/// handlers can run concurrently without `RefCell`.
pub(super) struct RhoAcpAgent {
    startup: AcpStartup,
    sessions: tokio::sync::Mutex<HashMap<SessionId, LiveSession>>,
}

impl RhoAcpAgent {
    pub(super) fn new(startup: AcpStartup) -> Self {
        Self {
            startup,
            sessions: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn initialize(request: &InitializeRequest) -> InitializeResponse {
        InitializeResponse::new(request.protocol_version)
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(
                        PromptCapabilities::new()
                            .image(true)
                            .audio(false)
                            .embedded_context(true),
                    ),
            )
            .auth_methods(Vec::new())
    }

    pub(super) fn authenticate(_request: &AuthenticateRequest) -> AcpError {
        not_yet_supported("authenticate")
    }

    pub(super) fn set_session_mode(request: &SetSessionModeRequest) -> AcpError {
        if super::permission::parse_mode_id(request.mode_id.0.as_ref()).is_none() {
            return AcpError::invalid_params().data(format!(
                "unknown session mode '{}'",
                request.mode_id.0.as_ref()
            ));
        }
        not_yet_supported("session/set_mode")
    }

    pub(super) async fn new_session(
        self: &Arc<Self>,
        request: NewSessionRequest,
    ) -> Result<NewSessionResponse, AcpError> {
        let ctx = self.build_context();
        let (host, response) = SessionHost::create(ctx, request)
            .await
            .map_err(host_error)?;
        let session_id = response.session_id.clone();
        let previous = {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(session_id, LiveSession::new(host))
        };
        shutdown_replaced(previous).await;
        Ok(response)
    }

    pub(super) async fn load_session(
        self: &Arc<Self>,
        request: LoadSessionRequest,
        port: &dyn AcpClientPort,
    ) -> Result<LoadSessionResponse, AcpError> {
        let session_id = request.session_id.clone();
        let ctx = self.build_context();
        let (host, response) = SessionHost::load(ctx, request, port)
            .await
            .map_err(host_error)?;
        let previous = {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(session_id, LiveSession::new(host))
        };
        shutdown_replaced(previous).await;
        Ok(response)
    }

    pub(super) async fn prompt(
        self: &Arc<Self>,
        request: PromptRequest,
        port: &dyn AcpClientPort,
    ) -> Result<PromptResponse, AcpError> {
        let session_id = request.session_id.clone();
        let mut host = {
            let mut sessions = self.sessions.lock().await;
            let live = sessions
                .get_mut(&session_id)
                .ok_or_else(|| missing_session(&session_id))?;
            live.host
                .take()
                .ok_or_else(|| missing_session(&session_id))?
        };
        let result = host.prompt(request, port).await;
        {
            let mut sessions = self.sessions.lock().await;
            match sessions.get_mut(&session_id) {
                Some(live) => live.host = Some(host),
                None => host.shutdown().await,
            }
        }
        result.map_err(host_error)
    }

    pub(super) async fn cancel(&self, notification: CancelNotification) {
        let sessions = self.sessions.lock().await;
        if let Some(live) = sessions.get(&notification.session_id) {
            live.cancel.cancel();
        }
    }

    pub(super) async fn shutdown_all(&self) {
        let lives: Vec<LiveSession> = {
            let mut sessions = self.sessions.lock().await;
            sessions.drain().map(|(_, live)| live).collect()
        };
        for live in lives {
            shutdown_replaced(Some(live)).await;
        }
    }

    fn build_context(&self) -> SessionBuildContext<'_> {
        SessionBuildContext {
            config: &self.startup.config,
            config_path: &self.startup.config_path,
            process_cwd: &self.startup.cwd,
            no_system_prompt: self.startup.no_system_prompt,
            no_tools: self.startup.no_tools,
            no_subagents: self.startup.no_subagents,
            agent: &self.startup.agent,
            diagnostics: &self.startup.diagnostics,
            herdr: &self.startup.herdr,
        }
    }
}

fn not_yet_supported(method: &str) -> AcpError {
    AcpError::new(
        i32::from(ErrorCode::MethodNotFound),
        format!("{method} is not yet supported"),
    )
}

fn missing_session(session_id: &SessionId) -> AcpError {
    AcpError::resource_not_found(Some(session_id.to_string()))
}

fn host_error(error: anyhow::Error) -> AcpError {
    AcpError::internal_error().data(error.to_string())
}

impl LiveSession {
    fn new(host: SessionHost) -> Self {
        Self {
            cancel: host.cancel_handle(),
            host: Some(host),
        }
    }
}

async fn shutdown_replaced(previous: Option<LiveSession>) {
    if let Some(live) = previous {
        live.cancel.cancel();
        if let Some(host) = live.host {
            host.shutdown().await;
        }
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
