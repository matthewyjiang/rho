//! The daemon session the TUI follows instead of running one in process
//! (#1667, behind `RHO_TUI_DAEMON=1`; see `app/interactive.rs`).
//!
//! [`DaemonSession::open`] opens or resumes the session on this process's
//! daemon connection (`app::daemon_client`, which starts the daemon when
//! none runs) and attaches to it before the TUI takes the terminal, so a
//! refusal reads as an ordinary startup error. The app loop then waits on
//! [`next_views`], which maps what the daemon sent since through
//! [`DaemonEvents`]: the reset replay of the saved conversation first, then
//! live events.
//!
//! Read-only so far: prompts and cancel come in T1c, input requests in T1d,
//! options in T1e. Quitting drops the connection, and the daemon keeps the
//! session; a detach, and reattaching after `lagged`, come in T1f.
//!
//! Off unix there is no daemon (#1665): the type is uninhabited, so the
//! TUI's daemon paths compile everywhere and never run there.

#[cfg(unix)]
pub(crate) use unix::{DaemonSession, DaemonTarget};

use super::daemon_events::DaemonView;

/// What the followed session changed since the last call, waiting until it
/// changes; never ready without a session.
pub(super) async fn next_views(session: &mut Option<DaemonSession>) -> Vec<DaemonView> {
    match session {
        Some(session) => session.next_views().await,
        None => std::future::pending().await,
    }
}

#[cfg(not(unix))]
pub(crate) enum DaemonSession {}

#[cfg(not(unix))]
impl DaemonSession {
    pub(crate) fn saved_id(&self) -> Option<&str> {
        match *self {}
    }

    pub(crate) fn workspace(&self) -> &std::path::Path {
        match *self {}
    }

    async fn next_views(&mut self) -> Vec<DaemonView> {
        match *self {}
    }
}

#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};

    use anyhow::Context;
    use rho_protocol::{
        methods::{SessionAttach, SessionNew},
        ClientEnvironment, DetachReason, Replay, SessionAttachParams, SessionId, SessionNewParams,
        SessionSetup,
    };
    use tokio::sync::mpsc;

    use super::super::daemon_events::{DaemonEvents, DaemonView};
    use crate::app::daemon_client::{Daemon, Routed};

    /// Which session the TUI follows.
    pub(crate) enum DaemonTarget {
        /// A new session the daemon opens with these params.
        New(SessionNewParams),
        /// The saved session `session`, which the daemon reopens with
        /// `setup` unless it has it loaded.
        Resume {
            session: SessionId,
            setup: SessionSetup,
        },
    }

    /// One daemon session the TUI follows.
    pub(crate) struct DaemonSession {
        /// Holds the connection; dropping it closes the connection, which
        /// detaches the session.
        _daemon: Daemon,
        id: SessionId,
        /// Whether the session was saved before the TUI opened it. A new one
        /// is saved once its first turn runs, which T1c brings.
        resumed: bool,
        workspace: PathBuf,
        routed: mpsc::UnboundedReceiver<Routed>,
        events: DaemonEvents,
        /// The reset replay's [`DaemonView::Clear`], shown with the first
        /// views.
        pending: Vec<DaemonView>,
        /// Set once the daemon ended the attachment; nothing more arrives.
        ended: bool,
    }

    impl DaemonSession {
        /// Opens or resumes `target` and attaches to it. The daemon starts
        /// when none runs.
        pub(crate) async fn open(target: DaemonTarget) -> anyhow::Result<Self> {
            let daemon = Daemon::default();
            let (id, setup, resumed) = match target {
                DaemonTarget::New(params) => {
                    let setup = params.setup.clone();
                    let created = daemon
                        .request::<SessionNew>(&params)
                        .await
                        .context("could not open a daemon session")?;
                    (created.session, setup, false)
                }
                DaemonTarget::Resume { session, setup } => (session, setup, true),
            };
            // Routed before the attach, so the replay that follows its
            // response is not dropped.
            let (sender, routed) = mpsc::unbounded_channel();
            daemon.route(
                &id,
                Box::new(move |routed| {
                    let _ = sender.send(routed);
                }),
            );
            let params = SessionAttachParams {
                session: id.clone(),
                after: None,
                environment: ClientEnvironment::of_this_process(),
                setup,
            };
            let response = match daemon.request::<SessionAttach>(&params).await {
                Ok(response) => response,
                Err(error) => {
                    daemon.unroute(&id);
                    return Err(anyhow::Error::new(error)
                        .context(format!("could not attach to daemon session {id}")));
                }
            };
            let mut events = DaemonEvents::default();
            let pending = match response.replay {
                Replay::Reset => vec![events.reset()],
                // A first attach has no cursor, so it always resets; T1f
                // reattaches after `lagged` with one.
                Replay::Tail => Vec::new(),
            };
            Ok(Self {
                _daemon: daemon,
                id,
                resumed,
                workspace: response.workspace,
                routed,
                events,
                pending,
                ended: false,
            })
        }

        /// The session's id, once it is saved: a resumed session's. A new
        /// one has nothing to resume until its first turn saves it.
        pub(crate) fn saved_id(&self) -> Option<&str> {
            self.resumed.then_some(self.id.as_str())
        }

        /// The directory the session works in, as the daemon reports it.
        pub(crate) fn workspace(&self) -> &Path {
            &self.workspace
        }

        /// What the daemon sent since the last call, mapped; waits for at
        /// least one notification. A replay arrives all at once, so it
        /// reads as one batch rather than one frame per event.
        ///
        /// Cancel-safe: nothing is taken before the first notification
        /// arrives, so the app loop may drop the wait for another branch.
        pub(super) async fn next_views(&mut self) -> Vec<DaemonView> {
            if self.ended {
                return std::future::pending().await;
            }
            let mut next = self.routed.recv().await;
            let mut views = std::mem::take(&mut self.pending);
            loop {
                match next {
                    Some(Routed::Event { event, .. }) => views.extend(self.events.map(event)),
                    Some(Routed::Detached(reason)) => {
                        views.push(self.end(detached(reason)));
                        return views;
                    }
                    // The route outlives the connection only until the
                    // router hands it `Lost`.
                    Some(Routed::Lost) | None => {
                        views.push(self.end(lost()));
                        return views;
                    }
                }
                match self.routed.try_recv() {
                    Ok(routed) => next = Some(routed),
                    // Empty, or closed, which the next call reads as lost.
                    Err(_) => return views,
                }
            }
        }

        fn end(&mut self, view: DaemonView) -> DaemonView {
            self.ended = true;
            view
        }
    }

    fn lost() -> DaemonView {
        DaemonView::Error(
            "could not follow the daemon session: lost the connection to the rho daemon".into(),
        )
    }

    fn detached(reason: DetachReason) -> DaemonView {
        let detail = match reason {
            DetachReason::Lagged => "this client fell behind; reopen the session to catch up",
            DetachReason::DaemonStopping => "the rho daemon is stopping",
            DetachReason::Deleted => "the session was deleted",
            // `Unknown`, or a reason a newer daemon sends.
            _ => "the rho daemon detached this client",
        };
        DaemonView::Error(format!("could not follow the daemon session: {detail}"))
    }
}
