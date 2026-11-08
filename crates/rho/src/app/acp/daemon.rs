//! This process's one connection to the rho daemon.
//!
//! Every ACP session attaches on the same connection: the daemon keeps one
//! attachment per session, not per connection. The connection opens on the
//! first request that needs it, starting the daemon when none runs, as
//! `rho run --detach` does.
//!
//! One task per connection drains its notifications in order and routes
//! them: a session's events and detach to that session's pump
//! (`sessions.rs`), a login's prompt and outcome to the login waiting on
//! them (`auth.rs`). When the daemon closes the connection, every route
//! hears [`Routed::Lost`], and the next request connects again.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, PoisonError,
    },
};

use agent_client_protocol::Error as AcpError;
use rho_protocol::{
    methods::Method, Client, ClientError, DaemonNotification, DetachReason, ErrorCode, LoginId,
    LoginOutcome, LoginPrompt, Notifications, Seq, SessionEvent, SessionId,
};
use tokio::sync::{mpsc, oneshot};

use crate::{app::daemon_cli, daemon::Connection};

/// What the router reads for a session.
pub(super) enum Routed {
    /// One event of the session's stream, with its sequence number.
    Event { seq: Seq, event: SessionEvent },
    /// The daemon ended the session's attachment.
    Detached(DetachReason),
    /// The connection the session was attached on closed.
    Lost,
}

/// Where the router hands what it reads for one session, in order.
pub(super) type SessionRoute = Box<dyn Fn(Routed) + Send>;

/// What the router hands the login in flight.
pub(super) enum LoginEvent {
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
}

/// The daemon as this process reaches it.
#[derive(Default)]
pub(super) struct Daemon {
    link: tokio::sync::Mutex<Option<Arc<Link>>>,
    routes: Arc<Mutex<Routes>>,
}

/// One open connection.
pub(super) struct Link {
    client: Client,
    alive: Arc<AtomicBool>,
    fences: mpsc::UnboundedSender<oneshot::Sender<()>>,
}

impl Daemon {
    /// The open connection, connecting first when there is none or the last
    /// one closed. The error names what failed, such as a daemon that could
    /// not start, with its log file.
    pub(super) async fn link(&self) -> Result<Arc<Link>, AcpError> {
        let mut current = self.link.lock().await;
        if let Some(link) = current.as_ref().filter(|link| link.is_alive()) {
            return Ok(Arc::clone(link));
        }
        let Connection {
            client,
            notifications,
            ..
        } = daemon_cli::connect()
            .await
            .map_err(|error| AcpError::internal_error().data(error.to_string()))?;
        let alive = Arc::new(AtomicBool::new(true));
        let (fences, fence_receiver) = mpsc::unbounded_channel();
        tokio::spawn(tokio::task::unconstrained(route(
            notifications,
            fence_receiver,
            Arc::clone(&self.routes),
            Arc::clone(&alive),
        )));
        let link = Arc::new(Link {
            client,
            alive,
            fences,
        });
        *current = Some(Arc::clone(&link));
        Ok(link)
    }

    /// The open connection, if there is one; never connects. For requests
    /// that matter only to an attachment that still delivers, such as a
    /// detach or a cancel: a closed connection already ended it.
    pub(super) async fn current(&self) -> Option<Arc<Link>> {
        let current = self.link.lock().await;
        current.as_ref().filter(|link| link.is_alive()).cloned()
    }

    /// Sends `M` on the open connection, connecting first if needed.
    pub(super) async fn request<M: Method>(
        &self,
        params: &M::Params,
    ) -> Result<M::Result, AcpError> {
        let link = self.link().await?;
        link.request::<M>(params)
            .await
            .map_err(|error| daemon_error(&error))
    }

    /// Routes session `session`'s notifications to `route`, replacing any
    /// earlier route.
    pub(super) fn route(&self, session: &SessionId, route: SessionRoute) {
        lock(&self.routes).sessions.insert(session.clone(), route);
    }

    /// Drops session `session`'s route; what the daemon still sends for it
    /// is discarded.
    pub(super) fn unroute(&self, session: &SessionId) {
        lock(&self.routes).sessions.remove(session);
    }

    /// Routes login notifications to `sender`, replacing an earlier login's
    /// route. A connection runs one login at a time (`auth.rs`).
    pub(super) fn route_login(&self, sender: mpsc::UnboundedSender<LoginEvent>) {
        lock(&self.routes).login = Some(sender);
    }

    pub(super) fn unroute_login(&self) {
        lock(&self.routes).login = None;
    }
}

impl Link {
    pub(super) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub(super) async fn request<M: Method>(
        &self,
        params: &M::Params,
    ) -> Result<M::Result, ClientError> {
        self.client.request::<M>(params).await
    }

    /// Returns once the router has routed every notification this
    /// connection read before the call, such as the events of an attachment
    /// whose detach already answered. Nothing to wait for on a closed
    /// connection.
    pub(super) async fn fence(&self) {
        let (done, fenced) = oneshot::channel();
        if self.fences.send(done).is_ok() {
            let _ = fenced.await;
        }
    }
}

/// The router: one per connection, so a session's notifications reach its
/// pump in the order the daemon sent them.
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
    alive: Arc<AtomicBool>,
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
    let Routes { sessions, login } = {
        let mut routes = lock(&routes);
        alive.store(false, Ordering::Release);
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

/// The ACP error for a daemon refusal.
pub(super) fn daemon_error(error: &ClientError) -> AcpError {
    let message = match error {
        ClientError::Rpc { error, .. } => error.message.clone(),
        other => other.to_string(),
    };
    let acp_error = match error.error_code() {
        Some(ErrorCode::InvalidParams | ErrorCode::SessionOpenFailed) => AcpError::invalid_params(),
        // As a failed turn's: the daemon's own message, which names how to
        // sign in and quotes neither a credential nor the provider, and no
        // data.
        Some(ErrorCode::SignInRequired) => {
            return AcpError::new(
                i32::from(agent_client_protocol::ErrorCode::AuthRequired),
                message,
            );
        }
        Some(ErrorCode::SessionNotFound | ErrorCode::SessionNotLoaded) => {
            AcpError::resource_not_found(None)
        }
        Some(
            ErrorCode::SessionBusy
            | ErrorCode::SessionInUse
            | ErrorCode::LoginBusy
            | ErrorCode::EnvironmentOverride
            | ErrorCode::InvalidRequest,
        ) => AcpError::invalid_request(),
        Some(ErrorCode::DaemonStopping) => {
            return AcpError::internal_error().data("rho daemon is stopping");
        }
        _ => AcpError::internal_error(),
    };
    acp_error.data(message)
}

/// Why a session's prompt failed when its connection closed under it.
pub(super) fn lost() -> AcpError {
    AcpError::internal_error().data("lost the connection to the rho daemon")
}
