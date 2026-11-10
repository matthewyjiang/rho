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
//! with says whether a turn runs, and the input requests still open show.
//!
//! The TUI prompts, cancels, steers, and withdraws steers through it
//! (`daemon_turn`), and answers and holds input requests (`daemon_answers`);
//! configures options through a background request queue. Quitting closes
//! the connection, which detaches the session without cancelling its turn.
//! A lagged attachment resumes after its last cursor, preserving the
//! transcript for a tail replay or replacing it for a reset. One the daemon
//! detached as restarting, for an upgrade or `rho daemon restart` (#1713),
//! attaches again on a new connection once the old one closed, which waits
//! for the restarted daemon, and resets; one that had saved nothing, which
//! the restarted daemon cannot reopen, is replaced by a new session. A
//! failed reattach ends following. The exit receipt offers to resume a session
//! once it is saved: resumed, titled, or past its first prompt.
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
    pub(in crate::tui) fn configure(
        &self,
        _change: rho_protocol::SessionChange,
    ) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) fn request_mcp(&self) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) fn goal(
        &self,
        _action: rho_protocol::SessionGoalAction,
    ) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) fn rename(&self, _title: String) -> anyhow::Result<()> {
        match *self {}
    }

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
        &mut self,
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

    pub(in crate::tui) async fn answer(
        &mut self,
        _id: rho_protocol::InputRequestId,
        _answer: rho_protocol::InputAnswer,
    ) {
        match *self {}
    }

    pub(in crate::tui) async fn hold(&mut self, _id: rho_protocol::InputRequestId) -> bool {
        match *self {}
    }
}

#[cfg(unix)]
mod unix {
    use std::{
        future::Future,
        path::{Path, PathBuf},
        pin::Pin,
        sync::Arc,
    };

    use anyhow::Context;
    use rho_protocol::ClientError;
    use rho_protocol::{
        methods::{
            SessionAnswer, SessionAttach, SessionCancel, SessionConfigure, SessionHold, SessionNew,
            SessionSteer, SessionUnsteer,
        },
        ClientEnvironment, ContentBlock, DetachReason, ErrorCode, InputAnswer, InputRequestId,
        SessionAnswerParams, SessionAttachParams, SessionAttachResponse, SessionCancelParams,
        SessionChange, SessionConfigureParams, SessionHoldParams, SessionId, SessionNewParams,
        SessionOptions, SessionSetup, SessionSteerParams, SessionUnsteerParams, SteerId,
        UnsteerOutcome,
    };
    use rho_sdk::SteeringId;
    use tokio::sync::{mpsc, oneshot, watch};

    use super::super::{
        daemon_events::{DaemonEvents, DaemonView, Rest, TurnState},
        daemon_inputs::AnswerRefusal,
        daemon_requests::{DaemonRequests, Target},
    };
    use crate::{
        app::daemon_client::{Daemon, Link, Part, Routed, StreamPosition},
        daemon::DAEMON_RESTART_TIMEOUT,
    };

    const LOST: &str = "lost the connection to the rho daemon";

    /// Shown while the TUI reconnects to a restarting daemon.
    const RECONNECTING: &str = "rho daemon restarting; reconnecting";

    type Reattach = Pin<Box<dyn Future<Output = anyhow::Result<Reattached>> + Send>>;

    /// What a reattach came back with.
    struct Reattached {
        response: SessionAttachResponse,
        /// After the daemon restarted: where the session attached again.
        relinked: Option<Relinked>,
    }

    /// The session's attachment after the daemon restarted.
    struct Relinked {
        /// The connection it attached on.
        link: Arc<Link>,
        /// The route its events now arrive on.
        routed: mpsc::UnboundedReceiver<Routed>,
        /// The session's id: a new one when the old one had saved nothing.
        session: SessionId,
    }

    /// Attaching a session again once the daemon restarted.
    struct Relink {
        daemon: Arc<Daemon>,
        /// The attach, after the session's cursor.
        params: SessionAttachParams,
        /// What opened the session, when it had saved nothing yet, and
        /// the options it ran with, which a new session in its place takes.
        opened: Option<(SessionNewParams, Option<SessionOptions>)>,
    }

    /// The changes that move a session from the options `from` to `to`, in
    /// the order they apply: a model first, since it decides which
    /// reasoning levels and fast mode the others may pick.
    fn changes_between(from: &SessionOptions, to: &SessionOptions) -> Vec<SessionChange> {
        let mut changes = Vec::new();
        if from.model.current != to.model.current {
            changes.push(SessionChange::Model {
                model: to.model.current.clone(),
            });
        }
        if let Some(reasoning) = to.reasoning.as_ref().filter(|reasoning| {
            from.reasoning.as_ref().map(|from| from.current) != Some(reasoning.current)
        }) {
            changes.push(SessionChange::Reasoning {
                level: reasoning.current,
            });
        }
        let fast =
            |options: &SessionOptions| options.fast_mode.as_ref().is_some_and(|fast| fast.enabled);
        if fast(from) != fast(to) {
            changes.push(SessionChange::FastMode { enabled: fast(to) });
        }
        if from.permission_mode.current != to.permission_mode.current {
            changes.push(SessionChange::PermissionMode {
                mode: to.permission_mode.current,
            });
        }
        changes
    }

    impl Relink {
        /// Waits for `old`, the connection the daemon detached the session
        /// on, to close, then attaches on the next one, which waits for the
        /// restarted daemon. Once the router handed the session's old route
        /// `Lost`, a new route belongs to the next connection.
        async fn run(self, old: Option<Arc<Link>>) -> anyhow::Result<Reattached> {
            let Self {
                daemon,
                params,
                opened,
            } = self;
            if let Some(old) = old {
                old.closed().await;
            }
            let routed = route(&daemon, &params.session);
            // A daemon that failed to come back resets the connections that
            // waited for it; the second try starts a new daemon.
            let link = match daemon.link().await {
                Ok(link) => link,
                Err(_) => daemon.link().await?,
            };
            match link.request::<SessionAttach>(&params).await {
                Ok(response) => Ok(Reattached {
                    response,
                    relinked: Some(Relinked {
                        link,
                        routed,
                        session: params.session,
                    }),
                }),
                Err(error) => match opened {
                    Some((mut opened, carried))
                        if error.error_code() == Some(ErrorCode::SessionNotFound) =>
                    {
                        daemon.unroute(&params.session);
                        opened.environment = ClientEnvironment::of_this_process();
                        let created = link.request::<SessionNew>(&opened).await?;
                        let session = created.session;
                        // The settings the person changed carry over; one the
                        // restarted daemon refuses keeps its default.
                        let changes = carried
                            .map(|carried| changes_between(&created.options, &carried))
                            .unwrap_or_default();
                        for change in changes {
                            let params = SessionConfigureParams {
                                session: session.clone(),
                                change,
                            };
                            if let Err(error) = link.request::<SessionConfigure>(&params).await {
                                tracing::warn!(%error, "could not carry a setting to the new daemon session");
                            }
                        }
                        let routed = route(&daemon, &session);
                        let attach = SessionAttachParams {
                            session: session.clone(),
                            after: None,
                            ..params
                        };
                        let response = link.request::<SessionAttach>(&attach).await?;
                        Ok(Reattached {
                            response,
                            relinked: Some(Relinked {
                                link,
                                routed,
                                session,
                            }),
                        })
                    }
                    _ => Err(error.into()),
                },
            }
        }
    }

    /// Routes session `id`'s notifications to the receiver this returns,
    /// replacing any earlier route.
    fn route(daemon: &Daemon, id: &SessionId) -> mpsc::UnboundedReceiver<Routed> {
        let (sender, routed) = mpsc::unbounded_channel();
        daemon.route(
            id,
            Box::new(move |routed| {
                let _ = sender.send(routed);
            }),
        );
        routed
    }

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
        daemon: Arc<Daemon>,
        id: SessionId,
        /// Whether the session is saved, so the exit receipt offers to
        /// resume it: it was resumed, has a title, or took a prompt.
        saved: bool,
        setup: SessionSetup,
        /// What opened the session, while it was opened new: a restarted
        /// daemon cannot reopen it until it saves, so it opens a new one.
        opened: Option<SessionNewParams>,
        workspace: PathBuf,
        routed: mpsc::UnboundedReceiver<Routed>,
        /// Where the session's requests go.
        target: watch::Sender<Target>,
        /// The session's options as the daemon last reported them.
        options: Option<SessionOptions>,
        events: DaemonEvents,
        /// Which of the attachment's events are its replay.
        position: StreamPosition,
        /// Views made outside the event stream, shown with the next views:
        /// the attach response's, such as the reset replay's
        /// [`DaemonView::Clear`], and a refused answer's or hold's.
        pending: Vec<DaemonView>,
        /// Set once the daemon ended the attachment; nothing more arrives.
        ended: bool,
        requests: Option<DaemonRequests>,
        /// Kept across cancelled `next_views` waits; never restart an attach
        /// merely because the TUI handled a key or redrew.
        reattach: Option<Reattach>,
    }

    impl DaemonSession {
        /// Opens or resumes `target` and attaches to it. The daemon starts
        /// when none runs.
        pub(crate) async fn open(target: DaemonTarget) -> anyhow::Result<Self> {
            let daemon = Arc::new(Daemon::default());
            let (id, setup, opened) = match target {
                DaemonTarget::New(params) => {
                    let created = daemon
                        .request::<SessionNew>(&params)
                        .await
                        .context("could not open a daemon session")?;
                    (created.session, params.setup.clone(), Some(params))
                }
                DaemonTarget::Resume { session, setup } => (session, setup, None),
            };
            // Routed before the attach, so the replay that follows its
            // response is not dropped.
            let routed = route(&daemon, &id);
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
            let link = daemon.current().await.context(LOST)?;
            let (target, requests) = watch::channel(Target::Attached {
                link,
                session: id.clone(),
            });
            let mut events = DaemonEvents::default();
            let pending = begin_replay(&mut events, &response);
            Ok(Self {
                position: StreamPosition::attached(&response),
                daemon,
                id,
                saved: opened.is_none() || response.title.is_some(),
                setup: params.setup,
                opened,
                workspace: response.workspace,
                routed,
                target,
                options: Some(response.options.clone()),
                events,
                pending,
                ended: false,
                requests: Some(DaemonRequests::new(requests)),
                reattach: None,
            })
        }

        /// The session's id, once a resume, saved title, or catalog lookup
        /// confirmed it has something to resume.
        pub(crate) fn saved_id(&self) -> Option<&str> {
            self.saved.then_some(self.id.as_str())
        }

        /// The directory the session works in, as the daemon reports it.
        pub(crate) fn workspace(&self) -> &Path {
            &self.workspace
        }

        /// What the daemon sent since the last call, mapped; waits for a
        /// notification or reattach response. Queued replay events read as
        /// one batch rather than one frame per event.
        ///
        /// Cancel-safe: the reattach future stays in the session, and no
        /// queued views are taken until the wait finishes.
        pub(super) async fn next_views(&mut self) -> Vec<DaemonView> {
            if self.ended {
                return std::future::pending().await;
            }
            if let Some(views) = self.finish_reattach().await {
                return views;
            }
            let first = tokio::select! {
                biased;
                routed = self.routed.recv() => routed,
                Some(error) = self.requests.as_mut().expect("attached session has requests").views.recv() => {
                    return vec![error];
                }
            };
            let mut views = std::mem::take(&mut self.pending);
            self.read(first, &mut views);
            views
        }

        /// What the daemon sent since the last call, mapped, without
        /// waiting: what a request's response already let through.
        pub(in crate::tui) fn take_views(&mut self) -> Vec<DaemonView> {
            let mut views = std::mem::take(&mut self.pending);
            if let Some(requests) = &mut self.requests {
                while let Ok(error) = requests.views.try_recv() {
                    views.push(error);
                }
            }
            if !self.ended && self.reattach.is_none() {
                if let Ok(first) = self.routed.try_recv() {
                    self.read(Some(first), &mut views);
                }
            }
            views
        }

        /// Waits for the reattach under way, if any, and takes over the
        /// attachment it came back with, or ends following when it failed;
        /// the views to show, `pending` first.
        async fn finish_reattach(&mut self) -> Option<Vec<DaemonView>> {
            let reattached = self.reattach.as_mut()?.await;
            self.reattach = None;
            let mut views = std::mem::take(&mut self.pending);
            match reattached {
                Ok(Reattached { response, relinked }) => {
                    if let Some(Relinked {
                        link,
                        routed,
                        session,
                    }) = relinked
                    {
                        let status = format!("reconnected to rho daemon {}", link.daemon().version);
                        views.push(DaemonView::Status(if session == self.id {
                            status
                        } else {
                            // It had saved nothing the restarted daemon could reopen.
                            format!("{status} (new session)")
                        }));
                        self.id = session.clone();
                        self.routed = routed;
                        self.target.send_replace(Target::Attached { link, session });
                    }
                    self.position.reattached(&response);
                    self.options = Some(response.options.clone());
                    self.saved |= response.title.is_some();
                    self.workspace = response.workspace.clone();
                    views.extend(begin_replay(&mut self.events, &response));
                }
                Err(error) => {
                    views.extend(self.end(DaemonView::Error(format!(
                        "could not reattach the daemon session: {error:#}"
                    ))));
                }
            }
            Some(views)
        }

        /// The params that attach the session again after its cursor.
        fn reattach_params(&self) -> SessionAttachParams {
            SessionAttachParams {
                session: self.id.clone(),
                after: self.position.cursor().cloned(),
                environment: ClientEnvironment::of_this_process(),
                setup: self.setup.clone(),
            }
        }

        /// Maps `first` and everything queued behind it into `views`;
        /// `None` is a closed route.
        fn read(&mut self, first: Option<Routed>, views: &mut Vec<DaemonView>) {
            let mut next = first;
            loop {
                match next {
                    Some(Routed::Event { seq, event }) => {
                        let part = self.position.observe(seq);
                        match &event {
                            rho_protocol::SessionEvent::TitleChanged { .. } => self.saved = true,
                            rho_protocol::SessionEvent::OptionsChanged { options } => {
                                self.options = Some((**options).clone());
                            }
                            _ => {}
                        }
                        views.extend(self.events.map(event));
                        if let Part::Replayed { left: 0, .. } = part {
                            views.extend(self.events.end_replay());
                        }
                    }
                    Some(Routed::Detached(DetachReason::Lagged)) => {
                        let daemon = Arc::clone(&self.daemon);
                        let params = self.reattach_params();
                        self.reattach = Some(Box::pin(async move {
                            // Lagged ends the subscription, not the connection.
                            // A lost connection still ends following, never sends
                            // prompts or answers on a new unseen attachment.
                            let link = daemon.current().await.context(LOST)?;
                            let response = link.request::<SessionAttach>(&params).await?;
                            Ok(Reattached {
                                response,
                                relinked: None,
                            })
                        }));
                        return;
                    }
                    Some(Routed::Detached(DetachReason::Restarting)) => {
                        views.push(DaemonView::Status(RECONNECTING.into()));
                        // The connection the session is attached on, taken
                        // now: the daemon closes it next.
                        let old = match self.target.send_replace(Target::Reconnecting) {
                            Target::Attached { link, .. } => Some(link),
                            Target::Reconnecting | Target::Ended => None,
                        };
                        let relink = Relink {
                            daemon: Arc::clone(&self.daemon),
                            params: self.reattach_params(),
                            // Only a session that saved nothing can be
                            // missing from the restarted daemon.
                            opened: self
                                .opened
                                .clone()
                                .filter(|_| !self.saved)
                                .map(|opened| (opened, self.options.clone())),
                        };
                        self.reattach = Some(Box::pin(async move {
                            tokio::time::timeout(DAEMON_RESTART_TIMEOUT, relink.run(old))
                                .await
                                .map_err(|_| {
                                    anyhow::anyhow!(
                                        "the rho daemon did not answer within \
                                         DAEMON_RESTART_TIMEOUT ({DAEMON_RESTART_TIMEOUT:?}) \
                                         of restarting"
                                    )
                                })?
                        }));
                        return;
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
            self.target.send_replace(Target::Ended);
            [
                error,
                DaemonView::Turn(TurnState::Resting(Rest::Unfollowed)),
            ]
        }

        /// The connection this session is attached on. A new connection
        /// would not carry the attachment, so what is sent on it would run
        /// unseen: a turn, or a steer the TUI then also keeps as a
        /// follow-up once it reads the connection's loss.
        async fn attached_link(&self) -> anyhow::Result<Arc<Link>> {
            match &*self.target.borrow() {
                Target::Attached { link, .. } if link.is_alive() => Ok(Arc::clone(link)),
                Target::Attached { .. } | Target::Reconnecting | Target::Ended => {
                    anyhow::bail!(LOST)
                }
            }
        }

        /// Starts a turn of `content`. Once this returns, the views of what
        /// the daemon sent before it answered, such as the turn's start, are
        /// waiting for [`Self::take_views`], and the session is saved: the
        /// daemon answers once the turn saved its prompt.
        pub(in crate::tui) async fn prompt(
            &mut self,
            content: Vec<ContentBlock>,
        ) -> anyhow::Result<()> {
            // A restart the TUI has not caught up with yet: attach again
            // first, so the prompt reaches the restarted daemon.
            let mut views = self.take_views();
            views.extend(self.finish_reattach().await.unwrap_or_default());
            self.pending.extend(views);
            self.attached_link().await?;
            let answer = self
                .requests
                .as_ref()
                .expect("attached session has requests")
                .prompt(content)?;
            // The requests queue sends it under one idempotency key, again
            // on the restarted daemon when this one refused it as stopping,
            // which may be after its turn already saved the prompt: the
            // daemon runs it once.
            self.await_answer(answer).await?;
            self.saved = true;
            Ok(())
        }

        /// Waits for a prompt's `answer` while reading what the daemon sends
        /// the session meanwhile, into `pending`: the prompt may queue behind
        /// a change that waits for the session to reattach after a restart,
        /// which only reading the session's detach brings about.
        async fn await_answer(
            &mut self,
            mut answer: oneshot::Receiver<anyhow::Result<()>>,
        ) -> anyhow::Result<()> {
            loop {
                if self.ended {
                    return answer.await.unwrap_or_else(|_| Err(anyhow::anyhow!(LOST)));
                }
                if self.reattach.is_some() {
                    tokio::select! {
                        biased;
                        sent = &mut answer => {
                            return sent.unwrap_or_else(|_| Err(anyhow::anyhow!(LOST)));
                        }
                        views = self.finish_reattach() => {
                            self.pending.extend(views.unwrap_or_default());
                        }
                    }
                } else {
                    tokio::select! {
                        biased;
                        sent = &mut answer => {
                            return sent.unwrap_or_else(|_| Err(anyhow::anyhow!(LOST)));
                        }
                        routed = self.routed.recv() => {
                            let mut views = Vec::new();
                            self.read(routed, &mut views);
                            self.pending.extend(views);
                        }
                    }
                }
            }
        }

        /// Request live inventory without waiting for the active turn.
        pub(in crate::tui) fn request_mcp(&self) -> anyhow::Result<()> {
            if self.ended {
                anyhow::bail!(LOST);
            }
            self.requests
                .as_ref()
                .expect("attached session has requests")
                .request_mcp()
        }

        /// Send a goal action without waiting for the active turn.
        pub(in crate::tui) fn goal(
            &self,
            action: rho_protocol::SessionGoalAction,
        ) -> anyhow::Result<()> {
            if self.ended {
                anyhow::bail!(LOST);
            }
            self.requests
                .as_ref()
                .expect("attached session has requests")
                .goal(action)
        }

        /// Rename the session without waiting for the active turn; its
        /// `TitleChanged` event reports the new title.
        pub(in crate::tui) fn rename(&self, title: String) -> anyhow::Result<()> {
            if self.ended {
                anyhow::bail!(LOST);
            }
            self.requests
                .as_ref()
                .expect("attached session has requests")
                .rename(title)
        }

        /// Queue a change without waiting for the active turn to release it.
        pub(in crate::tui) fn configure(
            &self,
            change: rho_protocol::SessionChange,
        ) -> anyhow::Result<()> {
            if self.ended {
                anyhow::bail!(LOST);
            }
            self.requests
                .as_ref()
                .expect("attached session has requests")
                .configure(change)
        }

        /// Asks the turn under way, if any, to stop. A closed connection
        /// has already ended what the TUI followed, so it asks nothing.
        pub(in crate::tui) async fn cancel(&self) -> anyhow::Result<()> {
            // Only on the connection the session is attached on: one the
            // TUI reconnects on has no turn of its to cancel yet.
            let Ok(link) = self.attached_link().await else {
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
                .attached_link()
                .await?
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
                .attached_link()
                .await?
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

        /// Answers input request `id` with `answer`. Once this returns, a
        /// refusal's views wait for [`Self::take_views`]: an error row, then
        /// the request again while it is still open. An answer the daemon
        /// took shows nothing: the prompt already closed.
        pub(in crate::tui) async fn answer(&mut self, id: InputRequestId, answer: InputAnswer) {
            self.events.answering(id);
            let params = SessionAnswerParams {
                session: self.id.clone(),
                request: id,
                answer,
            };
            let answered = match self.attached_link().await {
                Ok(link) => link.request::<SessionAnswer>(&params).await.map_err(Some),
                Err(_) => Err(None),
            };
            let refusal = match answered {
                Ok(_) => return,
                Err(Some(error)) if error.error_code() == Some(ErrorCode::InputNotOpen) => {
                    AnswerRefusal::NotOpen
                }
                Err(Some(error)) => {
                    AnswerRefusal::Failed(format!("{:#}", anyhow::Error::new(error)))
                }
                // Following ends once the TUI reads the connection's loss.
                Err(None) => AnswerRefusal::Failed(LOST.to_owned()),
            };
            let views = self.events.answer_refused(id, refusal);
            self.pending.extend(views);
        }

        /// Stops input request `id`'s timeout, as once the person starts on
        /// it, and says whether it needs no more holding: the daemon held
        /// it, it already closed (its resolution says how), or the
        /// connection is gone. Once a hold fails, its error row waits for
        /// [`Self::take_views`].
        pub(in crate::tui) async fn hold(&mut self, id: InputRequestId) -> bool {
            let params = SessionHoldParams {
                session: self.id.clone(),
                request: id,
            };
            // Without the attached connection there is nothing to hold for:
            // following ends once the TUI reads its loss.
            let Ok(link) = self.attached_link().await else {
                return true;
            };
            match link.request::<SessionHold>(&params).await {
                Ok(_) => true,
                Err(error) if error.error_code() == Some(ErrorCode::InputNotOpen) => true,
                Err(error) => {
                    self.pending.push(DaemonView::Error(format!(
                        "could not hold the request: {:#}",
                        anyhow::Error::new(error)
                    )));
                    false
                }
            }
        }
    }

    /// A tail keeps message/card state, but open prompts are rebuilt from
    /// the attach snapshot and replay, just as for a reset. Returns the
    /// views the response makes.
    fn begin_replay(
        events: &mut DaemonEvents,
        response: &SessionAttachResponse,
    ) -> Vec<DaemonView> {
        let mut views = vec![events.replay(response.replay)];
        views.push(DaemonView::Options(Box::new(response.options.clone())));
        if let Some(title) = &response.title {
            views.push(DaemonView::Title(title.clone()));
        }
        events.attached(
            response.inputs.clone(),
            response.state,
            response.turn.clone(),
        );
        if response.replay_events == 0 {
            views.extend(events.end_replay());
        }
        views
    }

    /// Why a steer, or its withdrawal, was refused, as the TUI says it.
    fn steer_refused(error: ClientError) -> anyhow::Error {
        if error.error_code() == Some(ErrorCode::NotSteerable) {
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
            DetachReason::DaemonStopping => "the rho daemon is stopping",
            DetachReason::Deleted => "the session was deleted",
            // `Unknown`, or a reason a newer daemon sends. `Lagged` never
            // reaches here: the session reattaches instead.
            _ => "the rho daemon detached this client",
        };
        DaemonView::Error(format!("could not follow the daemon session: {detail}"))
    }
}
