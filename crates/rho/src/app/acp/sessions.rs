//! The sessions this process has open, each attached to the daemon.
//!
//! The daemon owns every session; this process attaches to the ones its
//! host opens, loads, or resumes, and relays them. A session's events
//! reach one pump task, in order, through the router (`daemon.rs`), and the
//! requests the host sends about it (`session/prompt`, config changes)
//! hand the pump what it must sequence with those events as
//! [`Control`]s on the same channel.
//!
//! A pump moves through each attachment in three steps: the attach's
//! replay (history shown to a host that loads a session, missed events
//! after a reattach, or nothing), then the host's response, which the
//! request releases through a gate so no update names a session before
//! the host learns of it, then live events. When the attachment ends with
//! the connection or the daemon, the session goes stale and the next
//! request about it attaches again.

mod pump;

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, PoisonError,
    },
};

use agent_client_protocol::{
    schema::v1::{
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
    ClientEnvironment, Cursor, ErrorCode, InputRequest, JobState, Replay, SessionAttachParams,
    SessionAttachResponse, SessionCancelParams, SessionChange, SessionConfigureParams,
    SessionDeleteParams, SessionDetachParams, SessionId, SessionNewParams, SessionOptions,
    SessionSetup, StreamId, TurnOutcome,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use super::{
    daemon::{self, Inbound},
    host_features::Forms,
    inputs::InputContext,
    options, requests, AcpClientPort, Adapter,
};

/// What a session's requests hand its pump, in order with its events.
pub(super) enum Control {
    /// An attachment began; its replay follows.
    Attached(Attached),
    /// A prompt of this host's is about to start; see [`Waiter`].
    Arm(Waiter),
    /// The daemon answered a config change with these options. The pump
    /// tells the host what changed, then answers, so the host hears the
    /// update before the response, and only once even when the daemon's
    /// `OptionsChanged` arrived first.
    Options(SessionOptions, oneshot::Sender<()>),
}

/// An attachment's start, built from the attach response.
pub(super) struct Attached {
    stream: StreamId,
    replay: Replay,
    replay_events: u64,
    show: Show,
    state: JobState,
    inputs: Vec<InputRequest>,
    title: Option<String>,
    options: SessionOptions,
    /// Answered once the replay is through.
    done: Option<oneshot::Sender<()>>,
    /// Released once the host has the response that names the session.
    gate: Option<oneshot::Receiver<()>>,
}

/// What a pump does with an attachment's replay.
pub(super) enum Show {
    /// History for a host that loads the session, marked as replay and
    /// sent through the loading request's port.
    History(Arc<dyn AcpClientPort>),
    /// Events the host missed while detached, handled as live.
    Missed,
    /// Nothing: the host has it, or the session is new.
    Skip,
}

/// Called with the daemon's id for the user message of an ACP v2 prompt,
/// which acknowledges the prompt.
pub(super) type Accept = Box<dyn FnOnce(MessageId) + Send>;

/// A prompt of this host's, waiting for its turn to end.
///
/// The first user message after arming is the prompt's own: the host
/// already shows it, so it is not echoed; an ACP v2 prompt's [`Accept`]
/// reports it instead. The first turn end after that message ends the
/// prompt. A waiter whose prompt gave up (its receiver is gone) is dropped.
pub(super) struct Waiter {
    pub(super) outcome: oneshot::Sender<Result<TurnOutcome, AcpError>>,
    pub(super) accept: Arc<Mutex<Option<Accept>>>,
}

/// Whether a session's attachment still delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attachment {
    Attached,
    /// The connection closed or the daemon stopped; attach again.
    Stale,
    /// The daemon deleted the session.
    Deleted,
}

/// What a session's requests and its pump share.
struct Shared {
    attachment: Attachment,
    /// The options the host last heard of.
    options: SessionOptions,
    /// Where to resume the stream after a reattach.
    cursor: Option<Cursor>,
}

/// One session open on this host.
pub(super) struct Live {
    pub(super) id: SessionId,
    pub(super) inbound: mpsc::UnboundedSender<Inbound>,
    shared: Arc<Mutex<Shared>>,
    job: watch::Receiver<JobState>,
    setup: SessionSetup,
    /// Held while a prompt of this host's runs.
    pub(super) prompt: tokio::sync::Mutex<()>,
    /// Set by `session/cancel` until the prompt ends, so a prompt sent
    /// right after a cancel waits for the cancelled one.
    pub(super) cancelling: AtomicBool,
    attaching: tokio::sync::Mutex<()>,
    pump: JoinHandle<()>,
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
    /// A turn was in progress when the attachment began, as another
    /// client's may be.
    pub(super) busy: bool,
    gate: oneshot::Sender<()>,
}

impl Started {
    /// Lets the session's live updates through, once the host has the
    /// response that names it.
    pub(super) fn open(self) {
        let _ = self.gate.send(());
    }
}

/// How a host reopens a saved session.
pub(super) enum Reopen {
    /// `session/load`: replay the history through `port`.
    Load(Arc<dyn AcpClientPort>),
    /// `session/resume`: the host has the history.
    Resume,
}

impl Sessions {
    fn get(&self, id: &str) -> Option<Arc<Live>> {
        lock(&self.live).get(id).cloned()
    }

    fn remove(&self, id: &str) -> Option<Arc<Live>> {
        lock(&self.live).remove(id)
    }

    fn all(&self) -> Vec<Arc<Live>> {
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
            .ok_or_else(|| AcpError::resource_not_found(Some(id.0.to_string())))?;
        {
            // Held across the reattach, so concurrent requests reattach once.
            let _attaching = live.attaching.lock().await;
            let attachment = lock(&live.shared).attachment;
            match attachment {
                Attachment::Attached => {}
                Attachment::Stale => self.reattach(&live).await?,
                Attachment::Deleted => {
                    self.sessions.remove(&id.0);
                    return Err(AcpError::resource_not_found(Some(id.0.to_string())));
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
        let features = self.negotiated.get();
        let setup = self.launch.setup(
            &request.additional_directories,
            &request.mcp_servers,
            /*forms*/ matches!(features.forms, Forms::Available(_)),
        );
        let created = self
            .daemon
            .request::<SessionNew>(&SessionNewParams {
                cwd,
                environment: ClientEnvironment::of_this_process(),
                agent: self.launch.agent.clone(),
                setup: setup.clone(),
            })
            .await?;
        self.open(created.session, setup, Show::Skip).await
    }

    /// `session/load` and `session/resume`. A session already open here is
    /// closed first, so the host gets a fresh attachment.
    pub(super) async fn reopen(
        &self,
        session_id: &AcpSessionId,
        additional_directories: &[std::path::PathBuf],
        servers: &[McpServer],
        how: Reopen,
    ) -> Result<Started, AcpError> {
        if let Some(live) = self.sessions.remove(&session_id.0) {
            self.detach(&live, /*wait_idle*/ false).await;
            if let Ok(link) = self.daemon.link().await {
                link.fence().await;
            }
        }
        let features = self.negotiated.get();
        let setup = self.launch.setup(
            additional_directories,
            servers,
            /*forms*/ matches!(features.forms, Forms::Available(_)),
        );
        let show = match how {
            Reopen::Load(port) => Show::History(port),
            Reopen::Resume => Show::Skip,
        };
        self.open(SessionId::new(session_id.0.as_ref()), setup, show)
            .await
    }

    pub(super) async fn load_session(
        &self,
        request: &LoadSessionRequest,
        port: Arc<dyn AcpClientPort>,
    ) -> Result<Started, AcpError> {
        self.reopen(
            &request.session_id,
            &request.additional_directories,
            &request.mcp_servers,
            Reopen::Load(port),
        )
        .await
    }

    pub(super) async fn resume_session(
        &self,
        request: &ResumeSessionRequest,
    ) -> Result<Started, AcpError> {
        self.reopen(
            &request.session_id,
            &request.additional_directories,
            &request.mcp_servers,
            Reopen::Resume,
        )
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
        let features = self.negotiated.get();
        let acp_id = AcpSessionId::new(id.as_str());
        let (inbound, receiver) = mpsc::unbounded_channel();
        self.daemon.route(&id, inbound.clone());
        let response = match self.attach_request(&id, None, &setup).await {
            Ok(response) => response,
            Err(error) => {
                self.daemon.unroute(&id);
                return Err(error);
            }
        };
        let options = response.options.clone();
        let running = busy(response.state);
        let shared = Arc::new(Mutex::new(Shared {
            attachment: Attachment::Stale,
            options: options.clone(),
            cursor: None,
        }));
        let (job, job_receiver) = watch::channel(response.state);
        let context = InputContext {
            daemon: Arc::clone(&self.daemon),
            session: id.clone(),
            acp_id: acp_id.clone(),
            port,
            features,
            placeholders: Arc::clone(&self.sessions.placeholders),
        };
        let pump = pump::spawn(
            context,
            receiver,
            Arc::clone(&shared),
            setup.clone(),
            job,
            &self.herdr,
        );
        let (gate, gate_receiver) = oneshot::channel();
        let (attached, done) = Attached::new(response, show, Some(gate_receiver));
        let _ = inbound.send(Inbound::Session(Control::Attached(attached)));
        let live = Arc::new(Live {
            id,
            inbound,
            shared,
            job: job_receiver,
            setup,
            prompt: tokio::sync::Mutex::new(()),
            cancelling: AtomicBool::new(false),
            attaching: tokio::sync::Mutex::new(()),
            pump,
        });
        lock(&self.sessions.live).insert(acp_id.0.to_string(), Arc::clone(&live));
        if done.await.is_err() {
            self.sessions.remove(&acp_id.0);
            self.detach(&live, /*wait_idle*/ false).await;
            return Err(daemon::lost());
        }
        Ok(Started {
            session_id: acp_id,
            options,
            busy: running,
            gate,
        })
    }

    /// Attaches a stale session again, resuming after its cursor: the
    /// events it missed show as live, and a reset's history is skipped.
    async fn reattach(&self, live: &Live) -> Result<(), AcpError> {
        self.daemon.route(&live.id, live.inbound.clone());
        let cursor = lock(&live.shared).cursor.clone();
        let response = self.attach_request(&live.id, cursor, &live.setup).await?;
        let show = match response.replay {
            Replay::Tail => Show::Missed,
            Replay::Reset => Show::Skip,
        };
        let (attached, done) = Attached::new(response, show, /*gate*/ None);
        live.inbound
            .send(Inbound::Session(Control::Attached(attached)))
            .map_err(|_| daemon::lost())?;
        done.await.map_err(|_| daemon::lost())
    }

    async fn attach_request(
        &self,
        id: &SessionId,
        after: Option<Cursor>,
        setup: &SessionSetup,
    ) -> Result<SessionAttachResponse, AcpError> {
        self.daemon
            .request::<SessionAttach>(&SessionAttachParams {
                session: id.clone(),
                after,
                environment: ClientEnvironment::of_this_process(),
                setup: setup.clone(),
            })
            .await
    }

    /// Stops relaying `live`: cancels this host's prompt and waits for it
    /// (and, with `wait_idle`, for the cancelled turn's job to end), then
    /// detaches. The session stays in the daemon.
    async fn detach(&self, live: &Live, wait_idle: bool) {
        if live.prompt.try_lock().is_err() {
            self.cancel_live(live).await;
            drop(live.prompt.lock().await);
            if wait_idle {
                let mut job = live.job.clone();
                let _ = job.wait_for(|state| !busy(*state)).await;
            }
        }
        self.daemon.unroute(&live.id);
        live.pump.abort();
        if let Ok(link) = self.daemon.link().await {
            let params = SessionDetachParams {
                session: live.id.clone(),
            };
            if let Err(error) = link.request::<SessionDetach>(&params).await {
                tracing::debug!(%error, "could not detach an ACP session");
            }
        }
    }

    /// `session/close`: stops relaying the session; it stays saved.
    pub(super) async fn close_session(
        &self,
        request: &CloseSessionRequest,
    ) -> Result<CloseSessionResponse, AcpError> {
        if let Some(live) = self.sessions.remove(&request.session_id.0) {
            self.detach(&live, /*wait_idle*/ false).await;
        }
        Ok(CloseSessionResponse::new())
    }

    /// Detaches every session, as when the host disconnects.
    pub(super) async fn close_all(&self) {
        for live in self.sessions.all() {
            self.detach(&live, /*wait_idle*/ false).await;
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
            self.detach(&live, /*wait_idle*/ true).await;
        }
        let params = SessionDeleteParams {
            session: SessionId::new(request.session_id.0.as_ref()),
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
        let current = lock(&live.shared).options.clone();
        let change = options::change(request, &current, toggles)?;
        let options = self.configure(&live, change).await?;
        Ok(SetSessionConfigOptionResponse::new(options::config_options(
            &options, toggles,
        )))
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
        link.fence().await;
        let (told, heard) = oneshot::channel();
        let control = Control::Options(response.options.clone(), told);
        if live.inbound.send(Inbound::Session(control)).is_ok() {
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
        let params = SessionCancelParams {
            session: live.id.clone(),
        };
        if let Err(error) = self.daemon.request::<SessionCancel>(&params).await {
            tracing::warn!(%error, "could not cancel an ACP session's turn");
        }
    }
}

impl Attached {
    fn new(
        response: SessionAttachResponse,
        show: Show,
        gate: Option<oneshot::Receiver<()>>,
    ) -> (Self, oneshot::Receiver<()>) {
        let (done, replayed) = oneshot::channel();
        let attached = Self {
            stream: response.stream,
            replay: response.replay,
            replay_events: response.replay_events,
            show,
            state: response.state,
            inputs: response.inputs,
            title: response.title,
            options: response.options,
            done: Some(done),
            gate,
        };
        (attached, replayed)
    }
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
