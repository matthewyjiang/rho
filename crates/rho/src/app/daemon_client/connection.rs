//! One connection of this process to the rho daemon. Most frontends hold
//! one; the TUI opens another for the session it switches to, then drops
//! the old one.
//!
//! Every session the client attaches attaches on the same connection: the
//! daemon keeps one attachment per session, not per connection. The
//! connection opens on the first request that needs it, starting the daemon
//! when none runs, as `rho run --detach` does.
//!
//! One task per connection drains its notifications in order and routes
//! them: a session's events and detach to that session's route, a login's
//! prompt and outcome to the login waiting on them, and settings changes to
//! the settings route. When the daemon closes
//! the connection, every route hears [`Routed::Lost`], and the next request
//! connects again. A session the daemon detached as restarting routes
//! itself again once the connection closed ([`Link::closed`]), then attaches
//! on the next one, which waits for the restarted daemon.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use rho_protocol::{
    methods::Method, BuildSkew, Client, ClientError, ConfigChangedParams, DaemonInfo,
    DaemonNotification, DetachReason, LoginId, LoginOutcome, LoginPrompt, Notifications, Seq,
    SessionEvent, SessionId,
};
use rho_sdk::CancellationToken;
use tokio::sync::{mpsc, oneshot};

use crate::{
    app::daemon_cli::{self, Unreachable},
    daemon::{ConnectAs, Connection},
};

/// Why a request to the daemon failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DaemonError {
    /// No daemon could be reached to send it to.
    #[error(transparent)]
    Unreachable(#[from] Unreachable),
    /// The daemon refused it, or the connection failed under it.
    #[error(transparent)]
    Request(#[from] ClientError),
}

/// What the router reads for a session.
pub(crate) enum Routed {
    /// One event of the session's stream, with its sequence number.
    Event { seq: Seq, event: SessionEvent },
    /// The daemon ended the session's attachment.
    Detached(DetachReason),
    /// The connection the session was attached on closed.
    Lost,
}

/// Where the router hands what it reads for one session, in order.
pub(crate) type SessionRoute = Box<dyn Fn(Routed) + Send>;

/// What the router hands the login in flight.
pub(crate) enum LoginEvent {
    Prompt {
        login: LoginId,
        prompt: LoginPrompt,
    },
    Finished {
        login: LoginId,
        outcome: LoginOutcome,
    },
    Lost,
}

/// Where the router sends what it reads. Shared across connections, so a
/// session that reattaches after a reconnect keeps its route.
#[derive(Default)]
struct Routes {
    sessions: HashMap<SessionId, SessionRoute>,
    login: Option<mpsc::UnboundedSender<LoginEvent>>,
    /// Where `rho/config/changed` goes, for the settings this process read.
    config: Option<mpsc::UnboundedSender<ConfigChangedParams>>,
}

/// The daemon as this process reaches it. It has no default: each owner
/// says what its connections declare.
pub(crate) struct Daemon {
    link: tokio::sync::Mutex<Option<Arc<Link>>>,
    routes: Arc<Mutex<Routes>>,
    reach: daemon_cli::Reach,
    /// What its connections declare.
    connect_as: ConnectAs,
}

/// One open connection.
pub(crate) struct Link {
    client: Client,
    /// The daemon process the connection's hello reached.
    daemon: DaemonInfo,
    /// Set when that daemon is another build than this process.
    skew: Option<BuildSkew>,
    /// Cancelled by the router once the connection closed, after it handed
    /// every route [`Routed::Lost`].
    closed: CancellationToken,
    fences: mpsc::UnboundedSender<oneshot::Sender<()>>,
}

impl Daemon {
    /// A daemon for a person's frontend, which answers their sessions'
    /// approvals and questions, started when none runs.
    pub(crate) fn for_frontend() -> Self {
        Self::reaching(daemon_cli::Reach::StartIfNeeded, ConnectAs::Frontend)
    }

    /// A daemon reached as `reach` allows, by connections that declare
    /// what `connect_as` does.
    pub(crate) fn reaching(reach: daemon_cli::Reach, connect_as: ConnectAs) -> Self {
        Self {
            link: tokio::sync::Mutex::default(),
            routes: Arc::default(),
            reach,
            connect_as,
        }
    }

    /// The open connection, connecting first when there is none or the last
    /// one closed. The error names what failed, such as a daemon that could
    /// not start, with its log file.
    pub(crate) async fn link(&self) -> Result<Arc<Link>, Unreachable> {
        let mut current = self.link.lock().await;
        if let Some(link) = current.as_ref().filter(|link| link.is_alive()) {
            return Ok(Arc::clone(link));
        }
        let Connection {
            client,
            notifications,
            hello,
        } = daemon_cli::connect_with(self.reach, self.connect_as).await?;
        let closed = CancellationToken::new();
        let (fences, fence_receiver) = mpsc::unbounded_channel();
        tokio::spawn(tokio::task::unconstrained(route(
            notifications,
            fence_receiver,
            Arc::clone(&self.routes),
            closed.clone(),
        )));
        let link = Arc::new(Link {
            client,
            daemon: hello.daemon,
            skew: hello.skew,
            closed,
            fences,
        });
        *current = Some(Arc::clone(&link));
        Ok(link)
    }

    /// The open connection, if there is one; never connects. For requests
    /// that matter only to an attachment that still delivers, such as a
    /// detach or a cancel: a closed connection already ended it.
    pub(crate) async fn current(&self) -> Option<Arc<Link>> {
        let current = self.link.lock().await;
        current.as_ref().filter(|link| link.is_alive()).cloned()
    }

    /// Sends `M` on the open connection, connecting first if needed.
    pub(crate) async fn request<M: Method>(
        &self,
        params: &M::Params,
    ) -> Result<M::Result, DaemonError> {
        let link = self.link().await?;
        Ok(link.request::<M>(params).await?)
    }

    /// Routes session `session`'s notifications to `route`, replacing any
    /// earlier route.
    pub(crate) fn route(&self, session: &SessionId, route: SessionRoute) {
        lock(&self.routes).sessions.insert(session.clone(), route);
    }

    /// Drops session `session`'s route; what the daemon still sends for it
    /// is discarded.
    pub(crate) fn unroute(&self, session: &SessionId) {
        lock(&self.routes).sessions.remove(session);
    }

    /// Routes login notifications to `sender`, replacing an earlier login's
    /// route. A connection runs one login at a time.
    pub(crate) fn route_login(&self, sender: mpsc::UnboundedSender<LoginEvent>) {
        lock(&self.routes).login = Some(sender);
    }

    pub(crate) fn unroute_login(&self) {
        lock(&self.routes).login = None;
    }

    /// Routes settings changes to `sender`, replacing an earlier route.
    pub(crate) fn route_config(&self, sender: mpsc::UnboundedSender<ConfigChangedParams>) {
        lock(&self.routes).config = Some(sender);
    }
}

impl Link {
    pub(crate) fn is_alive(&self) -> bool {
        !self.closed.is_cancelled()
    }

    /// Resolves once the connection closed and the router handed every
    /// route it had [`Routed::Lost`], so a route installed after belongs to
    /// the next connection.
    pub(crate) async fn closed(&self) {
        self.closed.cancelled().await;
    }

    /// The daemon process this connection reached.
    pub(crate) fn daemon(&self) -> &DaemonInfo {
        &self.daemon
    }

    /// Set when the daemon this connection reached is another build than
    /// this process: requests may fail, and it says what a restart costs.
    pub(crate) fn skew(&self) -> Option<BuildSkew> {
        self.skew
    }

    pub(crate) async fn request<M: Method>(
        &self,
        params: &M::Params,
    ) -> Result<M::Result, ClientError> {
        self.client.request::<M>(params).await
    }

    /// Returns once the router has routed every notification this
    /// connection read before the call, such as the events of an attachment
    /// whose detach already answered. Nothing to wait for on a closed
    /// connection.
    pub(crate) async fn fence(&self) {
        let (done, fenced) = oneshot::channel();
        if self.fences.send(done).is_ok() {
            let _ = fenced.await;
        }
    }
}

/// The router: one per connection, so a session's notifications reach its
/// route in the order the daemon sent them.
///
/// The client's reader queues a response's notifications before it answers
/// the request, so once a response arrived, everything sent before it is
/// queued here. Polling notifications first (`biased`, and unconstrained so
/// the cooperative budget never makes a ready queue look empty) is what lets
/// a fence vouch for them.
async fn route(
    mut notifications: Notifications,
    mut fences: mpsc::UnboundedReceiver<oneshot::Sender<()>>,
    routes: Arc<Mutex<Routes>>,
    closed: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            notification = notifications.recv() => match notification {
                Some(notification) => deliver(&routes, notification),
                None => break,
            },
            Some(fence) = fences.recv() => {
                let _ = fence.send(());
            }
        }
    }
    // One step under the lock: a route installed after it belongs to the
    // next connection, which `link` opens only once this one is dead, so
    // this router never takes it.
    let Routes {
        sessions, login, ..
    } = {
        let mut routes = lock(&routes);
        closed.cancel();
        std::mem::take(&mut *routes)
    };
    for route in sessions.into_values() {
        route(Routed::Lost);
    }
    if let Some(login) = login {
        let _ = login.send(LoginEvent::Lost);
    }
}

fn deliver(routes: &Mutex<Routes>, notification: DaemonNotification) {
    let routes = lock(routes);
    let (session, routed) = match notification {
        DaemonNotification::SessionEvent(params) => (
            params.session,
            Routed::Event {
                seq: params.seq,
                event: params.event,
            },
        ),
        DaemonNotification::SessionDetached(params) => {
            (params.session, Routed::Detached(params.reason))
        }
        DaemonNotification::LoginPrompt(params) => {
            if let Some(login) = &routes.login {
                let _ = login.send(LoginEvent::Prompt {
                    login: params.login,
                    prompt: params.prompt,
                });
            }
            return;
        }
        DaemonNotification::LoginFinished(params) => {
            if let Some(login) = &routes.login {
                let _ = login.send(LoginEvent::Finished {
                    login: params.login,
                    outcome: params.outcome,
                });
            }
            return;
        }
        DaemonNotification::ConfigChanged(params) => {
            if let Some(config) = &routes.config {
                let _ = config.send(params);
            }
            return;
        }
        // `Unknown`, or a notification a newer protocol adds.
        _ => {
            tracing::debug!("skipping a daemon notification this rho does not know");
            return;
        }
    };
    if let Some(route) = routes.sessions.get(&session) {
        route(routed);
    }
}

fn lock(routes: &Mutex<Routes>) -> std::sync::MutexGuard<'_, Routes> {
    routes.lock().unwrap_or_else(PoisonError::into_inner)
}
