//! The daemon session the TUI follows instead of running one in process
//! (#1667, behind `RHO_TUI_DAEMON=1`; see `app/interactive.rs`).
//!
//! [`DaemonSession::open`] opens or resumes the session on this process's
//! daemon connection (`app::daemon_client`, which starts the daemon when
//! none runs) and attaches to it before the TUI takes the terminal, so a
//! refusal reads as an ordinary startup error. The app loop then waits on
//! [`next_views`], which maps what the daemon sent since through
//! [`DaemonEvents`]: the reset replay of the saved conversation first, then
//! live events. Once the replay is over, the job state the attach answered
//! with says whether a turn runs.
//!
//! The TUI prompts, cancels, steers, and withdraws steers through it
//! (`daemon_turn`); input requests come in T1d, options in T1e. Quitting
//! drops the connection, and the daemon keeps the session; a detach, and
//! reattaching after `lagged`, come in T1f.
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

    pub(in crate::tui) fn take_views(&mut self) -> Vec<DaemonView> {
        match *self {}
    }

    pub(in crate::tui) async fn prompt(
        &self,
        _content: Vec<rho_protocol::ContentBlock>,
    ) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) async fn cancel(&self) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) async fn steer(&self, _text: String) -> anyhow::Result<rho_sdk::SteeringId> {
        match *self {}
    }

    pub(in crate::tui) async fn unsteer(
        &self,
        _id: &rho_sdk::SteeringId,
    ) -> anyhow::Result<rho_protocol::UnsteerOutcome> {
        match *self {}
    }
}

#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};

    use anyhow::Context;
    use rho_protocol::{
        methods::{
            SessionAttach, SessionCancel, SessionNew, SessionPrompt, SessionSteer, SessionUnsteer,
        },
        ClientEnvironment, ContentBlock, DetachReason, ErrorCode, JobState, Replay,
        SessionAttachParams, SessionCancelParams, SessionId, SessionNewParams, SessionPromptParams,
        SessionSetup, SessionSteerParams, SessionUnsteerParams, SteerId, UnsteerOutcome,
    };
    use rho_sdk::SteeringId;
    use tokio::sync::mpsc;

    use super::super::daemon_events::{DaemonEvents, DaemonView, Rest, TurnState};
    use crate::app::daemon_client::{Daemon, Part, Routed, StreamPosition};

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
        daemon: Daemon,
        id: SessionId,
        /// Whether the session was saved before the TUI opened it. A new one
        /// is saved once its first turn runs.
        resumed: bool,
        workspace: PathBuf,
        routed: mpsc::UnboundedReceiver<Routed>,
        events: DaemonEvents,
        /// Which of the attachment's events are its replay.
        position: StreamPosition,
        /// The job state the attach answered with, which the replay ends in,
        /// until the replay is over.
        replay_state: Option<JobState>,
        /// Views the attach response itself made, such as the reset replay's
        /// [`DaemonView::Clear`], shown with the first views.
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
            let mut pending = match response.replay {
                Replay::Reset => vec![events.reset()],
                // A first attach has no cursor, so it always resets; T1f
                // reattaches after `lagged` with one.
                Replay::Tail => Vec::new(),
            };
            let mut replay_state = Some(response.state);
            if response.replay_events == 0 {
                pending.extend(replay_state.take().map(|state| events.turn(state)));
            }
            Ok(Self {
                position: StreamPosition::attached(&response),
                daemon,
                id,
                resumed,
                workspace: response.workspace,
                routed,
                events,
                replay_state,
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
            let first = self.routed.recv().await;
            let mut views = std::mem::take(&mut self.pending);
            self.read(first, &mut views);
            views
        }

        /// What the daemon sent since the last call, mapped, without
        /// waiting: what a request's response already let through.
        pub(in crate::tui) fn take_views(&mut self) -> Vec<DaemonView> {
            let mut views = std::mem::take(&mut self.pending);
            if !self.ended {
                if let Ok(first) = self.routed.try_recv() {
                    self.read(Some(first), &mut views);
                }
            }
            views
        }

        /// Maps `first` and everything queued behind it into `views`;
        /// `None` is a closed route.
        fn read(&mut self, first: Option<Routed>, views: &mut Vec<DaemonView>) {
            let mut next = first;
            loop {
                match next {
                    Some(Routed::Event { seq, event }) => {
                        let part = self.position.observe(seq);
                        views.extend(self.events.map(event));
                        if let Part::Replayed { left: 0, .. } = part {
                            views.extend(
                                self.replay_state
                                    .take()
                                    .map(|state| self.events.turn(state)),
                            );
                        }
                    }
                    Some(Routed::Detached(reason)) => {
                        views.extend(self.end(detached(reason)));
                        return;
                    }
                    // The route outlives the connection only until the
                    // router hands it `Lost`.
                    Some(Routed::Lost) | None => {
                        views.extend(self.end(lost()));
                        return;
                    }
                }
                match self.routed.try_recv() {
                    Ok(routed) => next = Some(routed),
                    // Empty, or closed, which the next call reads as lost.
                    Err(_) => return,
                }
            }
        }

        /// The attachment is over: `error` says why, and the TUI stops
        /// showing a turn it can no longer follow.
        fn end(&mut self, error: DaemonView) -> [DaemonView; 2] {
            self.ended = true;
            [
                error,
                DaemonView::Turn(TurnState::Resting(Rest::Unfollowed)),
            ]
        }

        /// Starts a turn of `content`. Once this returns, the views of what
        /// the daemon sent before it answered, such as the turn's start, are
        /// waiting for [`Self::take_views`].
        pub(in crate::tui) async fn prompt(
            &self,
            content: Vec<ContentBlock>,
        ) -> anyhow::Result<()> {
            // A new connection would not carry this attachment, so a turn
            // started on it would run unseen.
            let Some(link) = self.daemon.current().await.filter(|_| !self.ended) else {
                anyhow::bail!("lost the connection to the rho daemon");
            };
            let params = SessionPromptParams {
                session: self.id.clone(),
                content,
                timeout_ms: None,
                environment: ClientEnvironment::of_this_process(),
            };
            link.request::<SessionPrompt>(&params).await?;
            link.fence().await;
            Ok(())
        }

        /// Asks the turn under way, if any, to stop. A closed connection
        /// has already ended what the TUI followed, so it asks nothing.
        pub(in crate::tui) async fn cancel(&self) -> anyhow::Result<()> {
            let Some(link) = self.daemon.current().await else {
                return Ok(());
            };
            link.request::<SessionCancel>(&SessionCancelParams {
                session: self.id.clone(),
            })
            .await?;
            Ok(())
        }

        /// Hands `text` to the running turn's next model request; returns
        /// the steer's id, which its user message carries once it applies.
        pub(in crate::tui) async fn steer(&self, text: String) -> anyhow::Result<SteeringId> {
            let steered = self
                .daemon
                .request::<SessionSteer>(&SessionSteerParams {
                    session: self.id.clone(),
                    text,
                })
                .await;
            let id = match steered {
                Ok(response) => response.id,
                Err(error) => return Err(steer_refused(error)),
            };
            SteeringId::from_string(id.as_str())
                .map_err(|_| anyhow::anyhow!("the rho daemon answered with an empty steer id"))
        }

        /// Withdraws steer `id` unless the turn already took it.
        pub(in crate::tui) async fn unsteer(
            &self,
            id: &SteeringId,
        ) -> anyhow::Result<UnsteerOutcome> {
            let withdrawn = self
                .daemon
                .request::<SessionUnsteer>(&SessionUnsteerParams {
                    session: self.id.clone(),
                    id: SteerId::new(id.as_str()),
                })
                .await;
            match withdrawn {
                Ok(response) => Ok(response.outcome),
                Err(error) => Err(steer_refused(error)),
            }
        }
    }

    /// Why a steer, or its withdrawal, was refused, as the TUI says it.
    fn steer_refused(error: crate::app::daemon_client::DaemonError) -> anyhow::Error {
        let not_steerable = match &error {
            crate::app::daemon_client::DaemonError::Request(error) => {
                error.error_code() == Some(ErrorCode::NotSteerable)
            }
            crate::app::daemon_client::DaemonError::Unreachable(_) => false,
        };
        if not_steerable {
            anyhow::anyhow!("no model request is taking input")
        } else {
            anyhow::Error::new(error)
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
