use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use agent_client_protocol::{
    schema::v1::{
        AgentCapabilities, AuthenticateRequest, CancelNotification, InitializeRequest,
        InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
        NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse, SessionId,
        SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    },
    Error as AcpError,
};

use super::{
    session_host::{PromptGate, SessionHost},
    AcpClientPort, AcpStartup,
};

/// One ACP session slot. The host stays in this mutex for its whole life, so a
/// prompt never has to take it out of the map and put it back.
///
/// `try_lock` is the busy gate: the map lock is only held long enough to clone
/// this `Arc`. Cancel reads `cancel` without the host lock. A replacement marks
/// `replaced` and waits for any in-flight prompt to release the host lock before
/// publishing the new slot, so a follow-up prompt cannot run on the new host
/// while the old turn is still finishing.
struct LiveSession {
    host: tokio::sync::Mutex<Option<SessionHost>>,
    cancel: Arc<PromptGate>,
    replaced: Arc<AtomicBool>,
}

/// In-process ACP agent. Session slots live in a mutex map so request
/// handlers can run concurrently without `RefCell`.
pub(super) struct RhoAcpAgent {
    startup: AcpStartup,
    sessions: tokio::sync::Mutex<HashMap<SessionId, Arc<LiveSession>>>,
    /// Readers are `session/new` and `session/load` for their full
    /// build-and-publish lifecycle. `shutdown_all` takes the write lock so it
    /// waits for those requests, then sets `closed`. Prompt, cancel, and
    /// `publish` never take this lock.
    install_gate: tokio::sync::RwLock<()>,
    /// One lock per session ID. Replacements of the same ID stay ordered;
    /// different IDs publish and tear down independently.
    install_locks: tokio::sync::Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>,
    closed: AtomicBool,
}

impl RhoAcpAgent {
    pub(super) fn new(startup: AcpStartup) -> Self {
        Self {
            startup,
            sessions: tokio::sync::Mutex::new(HashMap::new()),
            install_gate: tokio::sync::RwLock::new(()),
            install_locks: tokio::sync::Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
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

    pub(super) async fn set_session_config_option(
        self: &Arc<Self>,
        request: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, AcpError> {
        let session_id = request.session_id.clone();
        let live = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&session_id)
                .cloned()
                .ok_or_else(|| missing_session(&session_id))?
        };
        let mut slot = live.try_lock_host(&session_id)?;
        slot.as_mut()
            .expect("try_lock_host rejects an empty slot")
            .set_config_option(request)
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
        let _gate = self.begin_install().await?;
        let (host, response) = SessionHost::create(&self.startup, request)
            .await
            .map_err(host_error)?;
        let session_id = response.session_id.clone();
        self.install(session_id, host).await;
        Ok(response)
    }

    pub(super) async fn load_session(
        self: &Arc<Self>,
        request: LoadSessionRequest,
        port: &dyn AcpClientPort,
    ) -> Result<LoadSessionResponse, AcpError> {
        let _gate = self.begin_install().await?;
        let session_id = request.session_id.clone();
        let (host, response) = SessionHost::load(&self.startup, request, port)
            .await
            .map_err(host_error)?;
        self.install(session_id, host).await;
        Ok(response)
    }

    pub(super) async fn prompt(
        self: &Arc<Self>,
        request: PromptRequest,
        port: &dyn AcpClientPort,
    ) -> Result<PromptResponse, AcpError> {
        let session_id = request.session_id.clone();
        let live = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&session_id)
                .cloned()
                .ok_or_else(|| missing_session(&session_id))?
        };
        let mut slot = live.try_lock_host(&session_id)?;
        slot.as_mut()
            .expect("try_lock_host rejects an empty slot")
            .prompt(request, port)
            .await
            .map_err(host_error)
    }

    pub(super) async fn cancel(&self, notification: CancelNotification) {
        let live = {
            let sessions = self.sessions.lock().await;
            sessions.get(&notification.session_id).cloned()
        };
        if let Some(live) = live {
            live.cancel.cancel();
        }
    }

    pub(super) async fn shutdown_all(&self) {
        let _gate = self.install_gate.write().await;
        self.closed.store(true, Ordering::Release);
        let lives: Vec<Arc<LiveSession>> = {
            let mut sessions = self.sessions.lock().await;
            sessions.drain().map(|(_, live)| live).collect()
        };
        self.install_locks.lock().await.clear();
        for live in lives {
            shutdown_live(live).await;
        }
    }

    async fn begin_install(&self) -> Result<tokio::sync::RwLockReadGuard<'_, ()>, AcpError> {
        let gate = self.install_gate.read().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(agent_stopped());
        }
        Ok(gate)
    }

    async fn install(&self, session_id: SessionId, host: SessionHost) {
        self.publish(session_id, LiveSession::new(host)).await;
    }

    async fn publish(&self, session_id: SessionId, live: Arc<LiveSession>) {
        let install = self.install_lock_for(&session_id).await;
        let _install = install.lock().await;
        if self.closed.load(Ordering::Acquire) {
            shutdown_live(live).await;
            return;
        }

        let previous = { self.sessions.lock().await.get(&session_id).cloned() };
        let old_host = if let Some(previous) = previous {
            previous.replaced.store(true, Ordering::Release);
            previous.cancel.cancel();
            previous.host.lock().await.take()
        } else {
            None
        };

        let outcome = {
            let mut sessions = self.sessions.lock().await;
            if self.closed.load(Ordering::Acquire) {
                Err(live)
            } else {
                sessions.insert(session_id, live);
                Ok(())
            }
        };
        if let Some(host) = old_host {
            host.shutdown().await;
        }
        if let Err(rejected) = outcome {
            shutdown_live(rejected).await;
        }
    }

    async fn install_lock_for(&self, session_id: &SessionId) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.install_locks.lock().await;
        locks
            .entry(session_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

/// The agent implements and advertises these methods, so `MethodNotFound`
/// would lie about the surface. `InvalidRequest` says the call cannot be
/// served as sent, and the data string says why.
fn not_yet_supported(method: &str) -> AcpError {
    AcpError::invalid_request().data(format!("{method} is not yet supported"))
}

fn missing_session(session_id: &SessionId) -> AcpError {
    AcpError::resource_not_found(Some(session_id.to_string()))
}

fn busy_session(session_id: &SessionId) -> AcpError {
    AcpError::invalid_request().data(format!(
        "session '{session_id}' already has an active prompt"
    ))
}

fn host_error(error: anyhow::Error) -> AcpError {
    AcpError::internal_error().data(error.to_string())
}

fn agent_stopped() -> AcpError {
    AcpError::internal_error().data("agent is shut down")
}

impl LiveSession {
    fn new(host: SessionHost) -> Arc<Self> {
        let cancel = host.cancel_handle();
        let replaced = host.replaced_flag();
        Arc::new(Self {
            host: tokio::sync::Mutex::new(Some(host)),
            cancel,
            replaced,
        })
    }

    fn try_lock_host(
        &self,
        session_id: &SessionId,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<SessionHost>>, AcpError> {
        let guard = self.host.try_lock().map_err(|_| busy_session(session_id))?;
        if guard.is_none() {
            return Err(busy_session(session_id));
        }
        Ok(guard)
    }
}

async fn shutdown_live(live: Arc<LiveSession>) {
    live.replaced.store(true, Ordering::Release);
    live.cancel.cancel();
    let host = live.host.lock().await.take();
    if let Some(host) = host {
        host.shutdown().await;
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
