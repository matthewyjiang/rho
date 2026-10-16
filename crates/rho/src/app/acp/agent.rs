use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use super::{
    session_host::{PromptGate, SessionHost},
    AcpClientPort, AcpStartup,
};
use agent_client_protocol::{
    schema::v1::{
        AgentCapabilities, AuthenticateRequest, CancelNotification, CloseSessionRequest,
        CloseSessionResponse, InitializeResponse, LoadSessionRequest, LoadSessionResponse,
        McpCapabilities, NewSessionRequest, NewSessionResponse, PromptCapabilities, PromptRequest,
        PromptResponse, SessionAdditionalDirectoriesCapabilities, SessionCapabilities,
        SessionCloseCapabilities, SessionId, SessionNotification, SetSessionConfigOptionRequest,
        SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse, StopReason,
    },
    schema::ProtocolVersion,
    Error as AcpError,
};

/// A published `session/new` or `session/load` result. `commands` is sent
/// after `response`, once the client knows the session.
#[derive(Debug)]
pub(super) struct SessionStart<Response> {
    pub(super) response: Response,
    pub(super) commands: SessionNotification,
}

/// One ACP session slot. The host stays in this mutex for its whole life, so a
/// prompt never has to take it out of the map and put it back.
///
/// The host lock is the busy gate: the map lock is only held long enough to
/// clone this `Arc`. [`HostWait`] decides who may queue for a held host.
/// Cancel reads `cancel` without the host lock. A replacement marks `replaced`
/// and waits for any in-flight prompt to release the host lock before
/// publishing the new slot, so a follow-up prompt cannot run on the new host
/// while the old turn is still finishing.
///
/// `session/close` instead unpublishes the slot and closes its `cancel` gate
/// before waiting for the host, so a prompt that wins the host after that
/// ends cancelled instead of starting a turn the close would wait on.
struct LiveSession {
    host: Arc<tokio::sync::Mutex<Option<SessionHost>>>,
    cancel: Arc<PromptGate>,
    replaced: Arc<AtomicBool>,
}

/// How a request waits when another request holds the session host.
#[derive(Clone, Copy, Debug)]
enum HostWait {
    /// A prompt queues behind a cancelled turn that is winding down, or a
    /// brief settings or teardown hold. Hosts that steer by interrupting
    /// (T3 Code) send `session/cancel` and the replacement prompt back to
    /// back; rejecting the second as busy would drop the user's message. A
    /// prompt behind a live turn is refused as busy.
    Prompt,
    /// A settings change (`session/set_mode`, `session/set_config_option`)
    /// queues behind anything, a live turn included, and applies once the
    /// host is free. ACP lets clients change settings mid-generation, but
    /// the SDK refuses setting changes during a run and a mode switch
    /// rebinds the runtime, so the change lands on the next prompt and the
    /// response waits for it. A change that only reaches the host after a
    /// replacement claimed it fails instead of applying there or following
    /// it: the retiring host is discarded, the successor starts from
    /// process defaults, and requests sent after the reload would reach the
    /// successor first, so moving the stale change over would apply it out
    /// of order. A change that gets the host before `session/load` claims
    /// it applies, and the load then resets to defaults.
    Settings,
}

/// What a request that waited for a session host got.
enum SessionHostLock {
    Ready(HostGuard),
    /// `session/close` claimed the session while the request waited.
    Closed,
}

/// In-process ACP agent. Session slots live in a mutex map so request
/// handlers can run concurrently without `RefCell`.
pub(super) struct RhoAcpAgent {
    startup: AcpStartup,
    sessions: tokio::sync::Mutex<HashMap<SessionId, Arc<LiveSession>>>,
    /// Readers are `session/new` and `session/load` for their full
    /// build-and-publish lifecycle, and `session/close` until its host is torn
    /// down. `shutdown_all` takes the write lock so it waits for those
    /// requests, then sets `closed`. Prompt, cancel, and `publish` never take
    /// this lock.
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

    /// Rho speaks ACP v1 only, so the answer does not depend on the request:
    /// any other requested version, including the pre-release v0 and T3's v2,
    /// gets v1 back and the client decides whether to continue.
    pub(super) fn initialize() -> InitializeResponse {
        InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(
                        PromptCapabilities::new()
                            .image(true)
                            .audio(false)
                            .embedded_context(true),
                    )
                    // Host stdio servers are the ACP baseline; Streamable
                    // HTTP is the transport Rho also speaks. SSE is not.
                    .mcp_capabilities(McpCapabilities::new().http(true))
                    // `additionalDirectories` on `session/new` and `session/load`,
                    // and `session/close`.
                    .session_capabilities(
                        SessionCapabilities::new()
                            .additional_directories(SessionAdditionalDirectoriesCapabilities::new())
                            .close(SessionCloseCapabilities::new()),
                    ),
            )
            .auth_methods(Vec::new())
            // Hosts display `version` as the agent version; it matches `rho --version`.
            .agent_info(
                agent_client_protocol::schema::v1::Implementation::new(
                    "rho",
                    env!("CARGO_PKG_VERSION"),
                )
                .title("Rho"),
            )
    }

    pub(super) fn authenticate(_request: &AuthenticateRequest) -> AcpError {
        not_yet_supported("authenticate")
    }

    /// Switches one session's permission mode for later prompts. Like
    /// config options, it waits for a running prompt to finish first.
    pub(super) async fn set_session_mode(
        self: &Arc<Self>,
        request: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse, AcpError> {
        let Some(mode) = super::permission::parse_mode_id(request.mode_id.0.as_ref()) else {
            return Err(AcpError::invalid_params().data(format!(
                "unknown session mode '{}'",
                request.mode_id.0.as_ref()
            )));
        };
        let mut host = self.lock_settings_host(&request.session_id).await?;
        host.set_permission_mode(mode, &self.startup.config).await?;
        Ok(SetSessionModeResponse::new())
    }

    pub(super) async fn new_session(
        self: &Arc<Self>,
        request: NewSessionRequest,
    ) -> Result<SessionStart<NewSessionResponse>, AcpError> {
        let _gate = self.begin_install().await?;
        let (host, response) = SessionHost::create(&self.startup, request)
            .await
            .map_err(session_start_error)?;
        let session_id = response.session_id.clone();
        let commands = host.available_commands(&self.startup.config);
        self.install(session_id, host).await;
        Ok(SessionStart { response, commands })
    }

    pub(super) async fn load_session(
        self: &Arc<Self>,
        request: LoadSessionRequest,
        port: &dyn AcpClientPort,
    ) -> Result<SessionStart<LoadSessionResponse>, AcpError> {
        let _gate = self.begin_install().await?;
        let session_id = request.session_id.clone();
        let install = self.install_lock_for(&session_id).await;
        let _install = install.lock().await;
        // A live session with this ID must stop saving before the replacement
        // reads the file, or the replacement's next save would drop steps the
        // old host checkpointed in between. Cancel its prompt, let that turn
        // save, then hold its host so nothing runs there until publication.
        let previous = { self.sessions.lock().await.get(&session_id).cloned() };
        let mut retired = None;
        if let Some(previous) = &previous {
            previous.cancel.cancel();
            let host = previous.host.lock().await;
            previous.replaced.store(true, Ordering::Release);
            retired = Some(host);
        }
        let (host, response) = match SessionHost::load(&self.startup, request, port).await {
            Ok(loaded) => loaded,
            Err(error) => {
                // The old session stays published, so it must keep saving.
                if let Some(previous) = &previous {
                    previous.replaced.store(false, Ordering::Release);
                }
                return Err(session_start_error(error));
            }
        };
        let commands = host.available_commands(&self.startup.config);
        let old_host = retired.and_then(|mut slot| slot.take());
        self.publish_locked(session_id, LiveSession::new(host), old_host)
            .await;
        Ok(SessionStart { response, commands })
    }

    pub(super) async fn prompt(
        self: &Arc<Self>,
        request: PromptRequest,
        port: &dyn AcpClientPort,
    ) -> Result<PromptResponse, AcpError> {
        let session_id = request.session_id.clone();
        match self
            .lock_session_host(&session_id, HostWait::Prompt)
            .await?
        {
            SessionHostLock::Ready(mut host) => host
                .prompt(request, port, &self.startup)
                .await
                .map_err(prompt_error),
            // A closing session ends a prompt that waited for its host as
            // `session/cancel` would, rather than failing it.
            SessionHostLock::Closed => Ok(PromptResponse::new(StopReason::Cancelled)),
        }
    }

    /// The host for `session_id`, once `wait` lets this request have it.
    async fn lock_session_host(
        &self,
        session_id: &SessionId,
        wait: HostWait,
    ) -> Result<SessionHostLock, AcpError> {
        let mut live = self.live_session(session_id).await?;
        loop {
            let locked = live.lock_host(session_id, wait).await;
            // A closing session's host is about to be torn down, so nothing
            // that waited for it may use it. One that passes here meets the
            // closed gate instead and starts cancelled.
            if live.cancel.is_closed() {
                return Ok(SessionHostLock::Closed);
            }
            if !live.replaced.load(Ordering::Acquire) {
                return locked.map(SessionHostLock::Ready);
            }
            match (wait, locked) {
                (HostWait::Settings, _) => return Err(reloaded_session(session_id)),
                (HostWait::Prompt, Ok(host)) => return Ok(SessionHostLock::Ready(host)),
                // A reload replaced the session while this prompt waited for
                // the old host; run it on the successor instead of dropping
                // the user's message.
                (HostWait::Prompt, Err(busy)) => {
                    live = self.successor(session_id, &live).await?.ok_or(busy)?;
                }
            }
        }
    }

    /// The host for a settings change. A session closed while the change
    /// waited is gone, like one that was never there.
    async fn lock_settings_host(&self, session_id: &SessionId) -> Result<HostGuard, AcpError> {
        match self
            .lock_session_host(session_id, HostWait::Settings)
            .await?
        {
            SessionHostLock::Ready(host) => Ok(host),
            SessionHostLock::Closed => Err(missing_session(session_id)),
        }
    }

    /// The session published in place of `replaced`, once the replacement
    /// holding this ID's install lock finishes. `None` when no other session
    /// took its place.
    async fn successor(
        &self,
        session_id: &SessionId,
        replaced: &Arc<LiveSession>,
    ) -> Result<Option<Arc<LiveSession>>, AcpError> {
        let install = self.install_lock_for(session_id).await;
        drop(install.lock().await);
        let next = self.live_session(session_id).await?;
        Ok((!Arc::ptr_eq(&next, replaced)).then_some(next))
    }

    async fn live_session(&self, session_id: &SessionId) -> Result<Arc<LiveSession>, AcpError> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| missing_session(session_id))
    }

    pub(super) async fn set_config_option(
        self: &Arc<Self>,
        request: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, AcpError> {
        let session_id = request.session_id.clone();
        let mut host = self.lock_settings_host(&session_id).await?;
        host.set_config_option(request, &self.startup.config).await
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

    /// `session/close`: unpublishes the session, cancels its prompt as
    /// `session/cancel` would, lets that turn save, then tears the host down
    /// with its MCP servers, background processes, and delegated agents. The
    /// saved session stays on disk, so a later `session/load` of the ID opens
    /// it again. An ID with no live session, including one already closed, is
    /// `resource_not_found` like every other session method; the spec lets an
    /// agent reject a session that does not exist or is not active.
    pub(super) async fn close_session(
        &self,
        request: CloseSessionRequest,
    ) -> Result<CloseSessionResponse, AcpError> {
        let _gate = self.begin_install().await?;
        let session_id = request.session_id;
        // Holding the ID's install lock through teardown keeps a `session/load`
        // of the same ID from reading the file before the cancelled turn saves.
        let install = self.install_lock_for(&session_id).await;
        let _install = install.lock().await;
        let live = self
            .sessions
            .lock()
            .await
            .remove(&session_id)
            .ok_or_else(|| missing_session(&session_id))?;
        shutdown_live(live).await;
        Ok(CloseSessionResponse::new())
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
        self.publish_locked(session_id, live, old_host).await;
    }

    /// Publishes `live` in place of the retired `old_host`. The caller holds
    /// this ID's install lock and has already emptied the previous slot.
    async fn publish_locked(
        &self,
        session_id: SessionId,
        live: Arc<LiveSession>,
        old_host: Option<SessionHost>,
    ) {
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

#[cfg(test)]
impl RhoAcpAgent {
    /// Publishes a prebuilt host, for fixtures that drive a real session
    /// through the agent's request handlers.
    pub(super) async fn install_for_tests(&self, session_id: SessionId, host: SessionHost) {
        self.install(session_id, host).await;
    }

    /// Holds the session's host as a running prompt would, plus the flag a
    /// replacement sets, so fixtures can race requests against a reload.
    pub(super) async fn hold_host_for_tests(
        &self,
        session_id: &SessionId,
    ) -> (
        tokio::sync::OwnedMutexGuard<Option<SessionHost>>,
        Arc<AtomicBool>,
    ) {
        let live = self.live_session(session_id).await.expect("live session");
        let held = Arc::clone(&live.host).lock_owned().await;
        (held, Arc::clone(&live.replaced))
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

pub(super) fn busy_session(session_id: &SessionId) -> AcpError {
    AcpError::invalid_request().data(format!(
        "session '{session_id}' already has an active prompt"
    ))
}

fn reloaded_session(session_id: &SessionId) -> AcpError {
    AcpError::invalid_request().data(format!(
        "session '{session_id}' was reloaded before this change applied; send it again"
    ))
}

/// Maps an SDK error from changing an idle session's settings: a busy
/// session is the host's to retry, anything else is internal.
pub(super) fn session_apply_error(session: &rho_sdk::Session, error: rho_sdk::Error) -> AcpError {
    match error {
        rho_sdk::Error::SessionBusy => busy_session(&SessionId::new(session.id().as_str())),
        error => AcpError::internal_error().data(error.to_string()),
    }
}

/// A command the prompt names but cannot run is the caller's to fix.
fn prompt_error(error: anyhow::Error) -> AcpError {
    match error.downcast::<super::commands::CommandRejected>() {
        Ok(rejected) => AcpError::invalid_params().data(rejected.to_string()),
        Err(error) => host_error(error),
    }
}

fn host_error(error: anyhow::Error) -> AcpError {
    AcpError::internal_error().data(error.to_string())
}

/// A lifecycle request naming a root the session cannot grant is the
/// caller's to fix.
fn session_start_error(error: anyhow::Error) -> AcpError {
    match error.downcast::<super::session_host::RejectedDirectory>() {
        Ok(rejected) => AcpError::invalid_params().data(rejected.to_string()),
        Err(error) => host_error(error),
    }
}

fn agent_stopped() -> AcpError {
    AcpError::internal_error().data("agent is shut down")
}

impl LiveSession {
    fn new(host: SessionHost) -> Arc<Self> {
        let cancel = host.cancel_handle();
        let replaced = host.replaced_flag();
        Arc::new(Self {
            host: Arc::new(tokio::sync::Mutex::new(Some(host))),
            cancel,
            replaced,
        })
    }

    fn try_lock_host(&self, session_id: &SessionId) -> Result<HostGuard, AcpError> {
        let guard = Arc::clone(&self.host)
            .try_lock_owned()
            .map_err(|_| busy_session(session_id))?;
        HostGuard::occupied(guard, session_id)
    }

    /// Like [`Self::try_lock_host`], but queues for a held host when `wait`
    /// allows it.
    async fn lock_host(
        &self,
        session_id: &SessionId,
        wait: HostWait,
    ) -> Result<HostGuard, AcpError> {
        let queues = match wait {
            HostWait::Prompt => self.cancel.admits_queued_prompt(),
            HostWait::Settings => true,
        };
        match self.try_lock_host(session_id) {
            Err(busy) if !queues => Err(busy),
            Err(_) => HostGuard::occupied(Arc::clone(&self.host).lock_owned().await, session_id),
            held => held,
        }
    }
}

/// Exclusive access to a live session host. [`HostGuard::occupied`] rejects
/// empty slots, so the Option unwrap is an invariant, not a caller concern.
struct HostGuard(tokio::sync::OwnedMutexGuard<Option<SessionHost>>);

const HOST_SLOT_INVARIANT: &str = "HostGuard::occupied rejects an empty slot";

impl HostGuard {
    /// A slot emptied by a replacement or shutdown has no host to lend.
    fn occupied(
        guard: tokio::sync::OwnedMutexGuard<Option<SessionHost>>,
        session_id: &SessionId,
    ) -> Result<Self, AcpError> {
        if guard.is_none() {
            return Err(busy_session(session_id));
        }
        Ok(Self(guard))
    }
}

impl Deref for HostGuard {
    type Target = SessionHost;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect(HOST_SLOT_INVARIANT)
    }
}

impl DerefMut for HostGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect(HOST_SLOT_INVARIANT)
    }
}

/// Cancels any prompt, closes the gate so no later prompt starts a turn, and
/// tears the host down. Shutdown has no successor host, so unlike a
/// replacement the cancelled turn still saves before the host lock is
/// released.
async fn shutdown_live(live: Arc<LiveSession>) {
    live.cancel.close();
    let host = live.host.lock().await.take();
    if let Some(host) = host {
        host.shutdown().await;
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
