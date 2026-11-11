//! The sessions this process has open, each attached to the daemon.
//!
//! The daemon owns every session; this process attaches to the ones its
//! host opens, loads, or resumes, and relays them. A session's events
//! reach one pump task (`sessions/pump.rs`), in order, through the router
//! (`app::daemon_client`), and the requests the host sends about it
//! (`session/prompt`, config changes) hand the pump what it must sequence
//! with those events as [`Control`]s on the same channel, after the events
//! the connection already read ([`Live::deliver`]).
//!
//! A pump moves through each attachment in three steps: the attach's
//! replay (history shown to a host that loads a session, missed events
//! after a reattach, or nothing), then the host's response, which the
//! request releases through a gate so no update names a session before
//! the host learns of it, then live events. When the attachment ends with
//! the connection or the daemon, the session goes stale, and the next
//! request about it has the pump attach again after its cursor.

mod pump;

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, PoisonError,
    },
};

use agent_client_protocol::{
    v1::schema::{
        CloseSessionRequest, CloseSessionResponse, DeleteSessionRequest, DeleteSessionResponse,
        LoadSessionRequest, McpServer, MessageId, NewSessionRequest, ResumeSessionRequest,
        SessionId as AcpSessionId, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
        SetSessionModeRequest, SetSessionModeResponse,
    },
    Error as AcpError,
};
use rho_protocol::{
    methods::{
        SessionAttach, SessionCancel, SessionConfigure, SessionDelete, SessionDetach, SessionNew,
    },
    ClientEnvironment, Cursor, ErrorCode, JobState, Replay, SessionAttachParams,
    SessionAttachResponse, SessionCancelParams, SessionChange, SessionConfigureParams,
    SessionDeleteParams, SessionDetachParams, SessionId, SessionNewParams, SessionOptions,
    SessionSetup, TurnId, TurnOutcome,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use super::{daemon, inputs::InputContext, options, requests, AcpClientPort, Adapter};
use crate::app::daemon_client::{Daemon, Link, Routed, SessionRoute};

/// What a session's pump reads, in order: what the router read for the
/// session, and what its requests hand it.
enum Inbound {
    Routed(Routed),
    Control(Control),
}

/// What a session's requests hand its pump.
pub(super) enum Control {
    /// Attach again after the cursor, once the last attachment ended;
    /// answered once the replay is through, or with why the attach failed.
    Resume(Replayed),
    /// A prompt of this host's is about to be sent; see [`Waiter`]. `turn`
    /// hears the turn the daemon answers it with, and closes unheard when
    /// the prompt started none. The pump waits for it before it reads on.
    Arm {
        waiter: Waiter,
        turn: oneshot::Receiver<TurnId>,
    },
    /// The daemon answered a config change with these options. The pump
    /// tells the host what changed, then answers, so the host hears the
    /// update before the response, and only once even when the daemon's
    /// `OptionsChanged` arrived first.
    Options(Box<SessionOptions>, oneshot::Sender<()>),
    /// Answered once the pump handled everything routed before it, so a
    /// response sent after the answer follows the updates sent before it.
    Flush(oneshot::Sender<()>),
}

/// Told once an attachment's replay is through, or why it never started.
pub(super) type Replayed = oneshot::Sender<Result<(), AcpError>>;

/// The attach that opened a session, which its pump starts on.
struct Opening {
    response: SessionAttachResponse,
    show: Show,
    replayed: Replayed,
    /// Released once the host has the response that names the session.
    gate: oneshot::Receiver<()>,
}

/// What a pump does with an attachment's replay.
#[derive(Clone)]
pub(super) enum Show {
    /// History for a host that loads the session, marked as replay and
    /// sent through the loading request's port.
    History(Arc<dyn AcpClientPort>),
    /// Events the host missed while detached, handled as live.
    Missed,
    /// Nothing: the host has it, or the session is new.
    Skip,
}

impl Show {
    /// What a reattach after the cursor shows of a `replay` replay: a tail
    /// holds the events the host missed, and a reset the history it has.
    fn resumed(replay: Replay) -> Self {
        match replay {
            Replay::Tail => Self::Missed,
            Replay::Reset => Self::Skip,
        }
    }
}

/// Called with the daemon's id for the user message of an ACP v2 prompt,
/// which acknowledges the prompt.
pub(super) type Accept = Box<dyn FnOnce(MessageId) + Send>;

/// A prompt of this host's, waiting for its turn to end.
///
/// The turn's first user message is the prompt's own: the host already
/// shows it, so it is not echoed; an ACP v2 prompt's [`Accept`] reports it
/// instead. The turn's end ends the prompt. A waiter whose prompt gave up
/// (its receiver is gone) is dropped.
pub(super) struct Waiter {
    pub(super) outcome: oneshot::Sender<Result<TurnOutcome, AcpError>>,
    pub(super) accept: Arc<Mutex<Option<Accept>>>,
}

/// Whether a session's attachment delivers, as its pump last saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attachment {
    /// Delivering; the session's job is in this state.
    Attached(JobState),
    /// Not attached yet, or the connection closed or the daemon stopped:
    /// attach again.
    Stale,
    /// The daemon deleted the session.
    Deleted,
}

impl Attachment {
    /// A turn of the session's is in progress, as far as this host knows:
    /// an attachment that no longer delivers reports none.
    fn busy(self) -> bool {
        matches!(self, Self::Attached(state) if busy(state))
    }
}

/// What a session's requests and its pump share.
struct Shared {
    /// The options the host last heard of.
    options: Mutex<SessionOptions>,
    attachment: watch::Sender<Attachment>,
}

/// Attaches sessions on the daemon for their pumps: the daemon itself, or a
/// fake in the pump's tests. Implementors answer with the daemon's response
/// or the ACP error a refused or failed attach answers.
pub(super) trait Attacher: Send + Sync {
    /// Attaches `session` with `setup`, resuming after `after` when the
    /// daemon still buffers it.
    fn attach<'a>(
        &'a self,
        session: &'a SessionId,
        after: Option<&'a Cursor>,
        setup: &'a SessionSetup,
    ) -> Pin<Box<dyn Future<Output = Result<SessionAttachResponse, AcpError>> + Send + 'a>>;
}

impl Attacher for Daemon {
    fn attach<'a>(
        &'a self,
        session: &'a SessionId,
        after: Option<&'a Cursor>,
        setup: &'a SessionSetup,
    ) -> Pin<Box<dyn Future<Output = Result<SessionAttachResponse, AcpError>> + Send + 'a>> {
        Box::pin(async move {
            let params = SessionAttachParams {
                session: session.clone(),
                after: after.cloned(),
                environment: ClientEnvironment::of_this_process(),
                setup: setup.clone(),
            };
            self.request::<SessionAttach>(&params)
                .await
                .map_err(AcpError::from)
        })
    }
}

/// One session open on this host.
pub(super) struct Live {
    pub(super) id: SessionId,
    inbound: mpsc::UnboundedSender<Inbound>,
    shared: Arc<Shared>,
    /// Held while a prompt of this host's runs.
    pub(super) prompt: tokio::sync::Mutex<()>,
    /// Set by `session/cancel` until the prompt ends, so a prompt sent
    /// right after a cancel waits for the cancelled one.
    pub(super) cancelling: AtomicBool,
    /// Set once the host closes the session, before its prompt is
    /// cancelled, so a prompt queued behind the cancelled one ends
    /// cancelled instead of starting a turn the close would wait out.
    pub(super) closing: AtomicBool,
    attaching: tokio::sync::Mutex<()>,
    pump: JoinHandle<()>,
}

impl Live {
    /// Hands the pump `control` after every event `link` already read for
    /// the session, so the pump handles them in order: once a daemon
    /// response arrived, after the events the daemon sent before it. Fails
    /// when the pump is gone, dropping `control` and any answer it carries.
    pub(super) async fn deliver(&self, link: &Link, control: Control) -> Result<(), AcpError> {
        link.fence().await;
        self.inbound
            .send(Inbound::Control(control))
            .map_err(|_| super::resource_not_found(Some(self.id.as_str().to_owned())))
    }
}

/// What closing a session waits for once it cancelled this host's prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// The prompt's end.
    Prompt,
    /// The prompt's end, then the cancelled turn's job's, so the session
    /// can be deleted.
    Job,
}

/// The sessions this process has open, by ACP session id.
#[derive(Default)]
pub(super) struct Sessions {
    live: Mutex<HashMap<String, Arc<Live>>>,
    placeholders: Arc<AtomicU64>,
}

/// A session the host just opened, before the host has its response.
pub(super) struct Started {
    pub(super) session_id: AcpSessionId,
    pub(super) options: SessionOptions,
    /// The session's job state when the attachment began: a turn another
    /// client started may be in progress.
    pub(super) state: JobState,
    gate: oneshot::Sender<()>,
}

impl Started {
    /// Lets the session's live updates through, once the host has the
    /// response that names it.
    pub(super) fn open(self) {
        let _ = self.gate.send(());
    }

    /// A turn was in progress when the attachment began.
    pub(super) fn busy(&self) -> bool {
        busy(self.state)
    }
}

impl Sessions {
    fn get(&self, id: &str) -> Option<Arc<Live>> {
        lock(&self.live).get(id).cloned()
    }

    fn remove(&self, id: &str) -> Option<Arc<Live>> {
        lock(&self.live).remove(id)
    }

    fn take_all(&self) -> Vec<Arc<Live>> {
        lock(&self.live).drain().map(|(_, live)| live).collect()
    }
}

impl Adapter {
    fn port(&self) -> Result<Arc<dyn AcpClientPort>, AcpError> {
        self.negotiated
            .client()
            .ok_or_else(|| AcpError::internal_error().data("ACP connection not initialized"))
    }

    /// The open session `id`, attached.
    pub(super) async fn live(&self, id: &AcpSessionId) -> Result<Arc<Live>, AcpError> {
        let live = self
            .sessions
            .get(&id.0)
            .ok_or_else(|| super::resource_not_found(Some(id.0.to_string())))?;
        {
            // Held across the reattach, so concurrent requests reattach once.
            let _attaching = live.attaching.lock().await;
            let attachment = *live.shared.attachment.borrow();
            match attachment {
                Attachment::Attached(_) => {}
                Attachment::Stale => self.resume(&live).await?,
                Attachment::Deleted => {
                    self.sessions.remove(&id.0);
                    self.forget(&live);
                    return Err(super::resource_not_found(Some(id.0.to_string())));
                }
            }
        }
        Ok(live)
    }

    /// `session/new`: a new daemon session, attached.
    pub(super) async fn new_session(
        &self,
        request: &NewSessionRequest,
    ) -> Result<Started, AcpError> {
        let cwd = requests::session_cwd(&request.cwd)?;
        let setup = self.launch.setup(
            &request.additional_directories,
            &request.mcp_servers,
            self.negotiated.get().forms,
        );
        let created = self
            .daemon
            .request::<SessionNew>(&SessionNewParams {
                cwd,
                environment: ClientEnvironment::of_this_process(),
                agent: self.launch.agent.clone(),
                setup: setup.clone(),
                parent: None,
                grant: None,
            })
            .await?;
        self.open(created.session, setup, Show::Skip).await
    }

    /// `session/load`: the saved session, its history replayed through
    /// `port`.
    pub(super) async fn load_session(
        &self,
        request: &LoadSessionRequest,
        port: Arc<dyn AcpClientPort>,
    ) -> Result<Started, AcpError> {
        self.reopen(
            &request.session_id,
            &request.additional_directories,
            &request.mcp_servers,
            Show::History(port),
        )
        .await
    }

    /// `session/resume`: the saved session, whose history the host has.
    pub(super) async fn resume_session(
        &self,
        request: &ResumeSessionRequest,
    ) -> Result<Started, AcpError> {
        self.reopen(
            &request.session_id,
            &request.additional_directories,
            &request.mcp_servers,
            Show::Skip,
        )
        .await
    }

    /// `session/load` and `session/resume`. A session already open here is
    /// closed first, so the host gets a fresh attachment.
    async fn reopen(
        &self,
        session_id: &AcpSessionId,
        additional_directories: &[std::path::PathBuf],
        servers: &[McpServer],
        show: Show,
    ) -> Result<Started, AcpError> {
        if let Some(live) = self.sessions.remove(&session_id.0) {
            self.detach(&live, Wait::Prompt).await;
            // The old attachment's events still in flight are routed, and
            // dropped, before the new one's route takes their place.
            if let Some(link) = self.daemon.current().await {
                link.fence().await;
            }
        }
        let setup = self
            .launch
            .setup(additional_directories, servers, self.negotiated.get().forms);
        self.open(SessionId::new(session_id.0.as_ref()), setup, show)
            .await
    }

    /// Attaches session `id` and starts its pump; returns once the replay
    /// is through.
    async fn open(
        &self,
        id: SessionId,
        setup: SessionSetup,
        show: Show,
    ) -> Result<Started, AcpError> {
        let port = self.port()?;
        let acp_id = AcpSessionId::new(id.as_str());
        let (inbound, receiver) = mpsc::unbounded_channel();
        self.daemon.route(&id, route(&inbound));
        let response = match self.daemon.attach(&id, /*after*/ None, &setup).await {
            Ok(response) => response,
            Err(error) => {
                self.daemon.unroute(&id);
                return Err(error);
            }
        };
        let options = response.options.clone();
        let state = response.state;
        let shared = Arc::new(Shared {
            options: Mutex::new(options.clone()),
            attachment: watch::channel(Attachment::Stale).0,
        });
        let context = InputContext {
            daemon: Arc::clone(&self.daemon),
            session: id.clone(),
            acp_id: acp_id.clone(),
            port,
            features: self.negotiated.get(),
            placeholders: Arc::clone(&self.sessions.placeholders),
        };
        let (replayed, through) = oneshot::channel();
        let (gate, gate_receiver) = oneshot::channel();
        let attacher: Arc<dyn Attacher> = self.daemon.clone();
        let pump = pump::spawn(
            context,
            attacher,
            receiver,
            Arc::clone(&shared),
            setup,
            &self.herdr,
            Opening {
                response,
                show,
                replayed,
                gate: gate_receiver,
            },
        );
        let live = Arc::new(Live {
            id,
            inbound,
            shared,
            prompt: tokio::sync::Mutex::new(()),
            cancelling: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            attaching: tokio::sync::Mutex::new(()),
            pump,
        });
        lock(&self.sessions.live).insert(acp_id.0.to_string(), Arc::clone(&live));
        if let Err(error) = through.await.unwrap_or_else(|_| Err(daemon::lost())) {
            self.sessions.remove(&acp_id.0);
            self.detach(&live, Wait::Prompt).await;
            return Err(error);
        }
        Ok(Started {
            session_id: acp_id,
            options,
            state,
            gate,
        })
    }

    /// Has a stale session's pump attach again after its cursor: the events
    /// the host missed show as live, and a reset's history is skipped.
    /// Returns once the replay is through.
    async fn resume(&self, live: &Live) -> Result<(), AcpError> {
        self.daemon.route(&live.id, route(&live.inbound));
        let (replayed, through) = oneshot::channel();
        live.inbound
            .send(Inbound::Control(Control::Resume(replayed)))
            .map_err(|_| daemon::lost())?;
        through.await.unwrap_or_else(|_| Err(daemon::lost()))
    }

    /// Stops relaying `live`: cancels this host's prompt and waits as
    /// `wait` says, then detaches. The session stays in the daemon.
    async fn detach(&self, live: &Live, wait: Wait) {
        // Before the cancel, so a prompt queued behind the cancelled one
        // sees it once it gets the lock.
        live.closing.store(true, Ordering::Release);
        if live.prompt.try_lock().is_err() {
            self.cancel_live(live).await;
            drop(live.prompt.lock().await);
            if wait == Wait::Job {
                let mut attachment = live.shared.attachment.subscribe();
                let _ = attachment.wait_for(|attachment| !attachment.busy()).await;
            }
        }
        let attachment = *live.shared.attachment.borrow();
        self.forget(live);
        // A stale or deleted attachment has nothing left to detach.
        if !matches!(attachment, Attachment::Attached(_)) {
            return;
        }
        if let Some(link) = self.daemon.current().await {
            let params = SessionDetachParams {
                session: live.id.clone(),
            };
            if let Err(error) = link.request::<SessionDetach>(&params).await {
                tracing::debug!(%error, "could not detach an ACP session");
            }
        }
    }

    /// Stops `live`'s routing and pump; what the daemon still sends for it
    /// is dropped.
    fn forget(&self, live: &Live) {
        self.daemon.unroute(&live.id);
        live.pump.abort();
    }

    /// `session/close`: stops relaying the session; it stays saved.
    pub(super) async fn close_session(
        &self,
        request: &CloseSessionRequest,
    ) -> Result<CloseSessionResponse, AcpError> {
        if let Some(live) = self.sessions.remove(&request.session_id.0) {
            self.detach(&live, Wait::Prompt).await;
        }
        Ok(CloseSessionResponse::new())
    }

    /// Detaches every session, as when the host disconnects or this
    /// process is told to stop.
    pub(super) async fn close_all(&self) {
        for live in self.sessions.take_all() {
            self.detach(&live, Wait::Prompt).await;
        }
        let _ = self.herdr.release().await;
    }

    /// `session/delete`. Hosts such as Zed delete from history without
    /// closing the thread, so a session open here is closed first, after
    /// its turn ends. Deleting converges: a session that is already gone,
    /// or never was, is deleted.
    pub(super) async fn delete_session(
        &self,
        request: &DeleteSessionRequest,
    ) -> Result<DeleteSessionResponse, AcpError> {
        if let Some(live) = self.sessions.remove(&request.session_id.0) {
            self.detach(&live, Wait::Job).await;
        }
        let params = SessionDeleteParams {
            session: SessionId::new(request.session_id.0.as_ref()),
            unless_attached: false,
        };
        let link = self.daemon.link().await?;
        match link.request::<SessionDelete>(&params).await {
            Ok(_) => {}
            Err(error) if error.error_code() == Some(ErrorCode::SessionNotFound) => {}
            Err(error) => return Err(daemon::daemon_error(&error)),
        }
        Ok(DeleteSessionResponse::new())
    }

    /// `session/set_config_option`.
    pub(super) async fn set_config_option(
        &self,
        request: &SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, AcpError> {
        let live = self.live(&request.session_id).await?;
        let toggles = self.negotiated.get().toggles;
        let current = lock(&live.shared.options).clone();
        let change = options::change(request, &current, toggles)?;
        let options = self.configure(&live, change).await?;
        Ok(SetSessionConfigOptionResponse::new(
            options::config_options(&options, toggles),
        ))
    }

    /// `session/set_mode`.
    pub(super) async fn set_session_mode(
        &self,
        request: &SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse, AcpError> {
        let live = self.live(&request.session_id).await?;
        let change = options::mode_change(&request.mode_id.0)?;
        self.configure(&live, change).await?;
        Ok(SetSessionModeResponse::new())
    }

    async fn configure(
        &self,
        live: &Live,
        change: SessionChange,
    ) -> Result<SessionOptions, AcpError> {
        let link = self.daemon.link().await?;
        let params = SessionConfigureParams {
            session: live.id.clone(),
            change,
        };
        let response = link
            .request::<SessionConfigure>(&params)
            .await
            .map_err(|error| daemon::daemon_error(&error))?;
        let (told, heard) = oneshot::channel();
        let control = Control::Options(Box::new(response.options.clone()), told);
        // A session closed meanwhile has no host to tell.
        if live.deliver(&link, control).await.is_ok() {
            let _ = heard.await;
        }
        Ok(response.options)
    }

    /// `session/cancel`: asks the daemon to stop the session's turn.
    pub(super) async fn cancel(&self, session_id: &AcpSessionId) {
        if let Some(live) = self.sessions.get(&session_id.0) {
            self.cancel_live(&live).await;
        }
    }

    async fn cancel_live(&self, live: &Live) {
        live.cancelling.store(true, Ordering::Release);
        // A closed connection already failed the prompt; the turn runs on in
        // the daemon, as when this process is killed.
        let Some(link) = self.daemon.current().await else {
            return;
        };
        let params = SessionCancelParams {
            session: live.id.clone(),
        };
        if let Err(error) = link.request::<SessionCancel>(&params).await {
            tracing::warn!(%error, "could not cancel an ACP session's turn");
        }
    }
}

/// The router's route to a pump reading `inbound`.
fn route(inbound: &mpsc::UnboundedSender<Inbound>) -> SessionRoute {
    let inbound = inbound.clone();
    Box::new(move |routed| {
        let _ = inbound.send(Inbound::Routed(routed));
    })
}

/// A turn is in progress: running, queued, or waiting for an answer.
fn busy(state: JobState) -> bool {
    matches!(
        state,
        JobState::Running | JobState::Queued | JobState::WaitingForInput
    )
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
