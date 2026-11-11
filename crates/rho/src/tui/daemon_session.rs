//! The daemon session the TUI follows instead of running one in process
//! (#1667, behind `RHO_TUI_DAEMON=1`; see `app/interactive.rs`).
//!
//! [`DaemonSession::open`] opens or resumes the session on a daemon
//! connection of its own (`app::daemon_client`, which starts the daemon
//! when none runs) and attaches to it: at launch before the TUI takes the
//! terminal, so a refusal reads as an ordinary startup error, and again
//! for each session the TUI switches to (`daemon_switch`). The app loop then waits on
//! [`next_views`], which maps what the daemon sent since through
//! [`DaemonEvents`]: the reset replay of the saved conversation first, then
//! live events. Once the replay is over, the job state the attach answered
//! with says whether a turn runs, and the input requests still open show.
//!
//! The TUI prompts, cancels, steers, and withdraws steers through it
//! (`daemon_turn`), and answers and holds input requests (`daemon_answers`);
//! configures options through a background request queue. Once attached,
//! it registers the `tui` client's settings and reads the daemon's
//! (`daemon_settings`), whose changes then arrive with the session's views;
//! settings writes bypass that queue. Quitting closes
//! the connection, which detaches the session without cancelling its turn.
//! A lagged attachment resumes after its last cursor, preserving the
//! transcript for a tail replay or replacing it for a reset. One the daemon
//! detached as restarting, for an upgrade or `rho daemon restart` (#1713),
//! attaches again on a new connection once the old one closed, which waits
//! for the restarted daemon, and resets; one that had saved nothing, which
//! the restarted daemon cannot reopen, is replaced by a new session, as is
//! one opened without saving, whose conversation is lost with it. A
//! failed reattach ends following. The exit receipt offers to resume a session
//! once it is saved: resumed, titled, past its first prompt, or given
//! context (`/remember`, a `!` command's output).
//!
//! Off unix there is no daemon (#1665): the type is uninhabited, so the
//! TUI's daemon paths compile everywhere and never run there.

#[cfg(unix)]
pub(crate) use unix::DaemonSession;

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
#[path = "daemon_session_absent.rs"]
mod absent;
#[cfg(not(unix))]
pub(crate) use absent::DaemonSession;

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
        ClientEnvironment, ConfigChangedParams, ContentBlock, DetachReason, ErrorCode, InputAnswer,
        InputRequestId, JobState, SessionAnswerParams, SessionAttachParams, SessionAttachResponse,
        SessionCancelParams, SessionChange, SessionConfigureParams, SessionHoldParams, SessionId,
        SessionNewParams, SessionOptions, SessionSteerParams, SessionUnsteerParams, SteerId,
        UnsteerOutcome,
    };
    use rho_sdk::SteeringId;
    use tokio::sync::{mpsc, oneshot, watch};

    use super::super::daemon_launch::{DaemonLaunch, DaemonTarget, InitialSettings, LaunchChange};
    use super::super::{
        daemon_events::{DaemonEvents, DaemonView, LoginUpdate, Rest, TurnState},
        daemon_inputs::AnswerRefusal,
        daemon_options::setting_name,
        daemon_requests::{DaemonRequests, Target},
        daemon_settings::DaemonSettings,
        daemon_settings_read::{load_settings, route_config, settings_loaded},
    };
    use crate::{
        app::daemon_client::{Daemon, Link, LoginEvent, Part, Routed, StreamPosition},
        daemon::DAEMON_RESTART_TIMEOUT,
    };

    mod auth;
    mod requests;

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
        /// The settings, read again: the restarted daemon knows neither
        /// this connection's subscription nor the TUI's registration.
        config_changes: mpsc::UnboundedReceiver<ConfigChangedParams>,
        settings: anyhow::Result<DaemonSettings>,
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
        if from.model.current != to.model.current || from.model.auth != to.model.auth {
            changes.push(SessionChange::Model {
                model: to.model.current.clone(),
                auth: Some(to.model.auth.clone()),
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
            let config_changes = route_config(&daemon);
            // A daemon that failed to come back resets the connections that
            // waited for it; the second try starts a new daemon.
            let link = match daemon.link().await {
                Ok(link) => link,
                Err(_) => daemon.link().await?,
            };
            let settings = load_settings(&link).await;
            match link.request::<SessionAttach>(&params).await {
                Ok(response) => Ok(Reattached {
                    response,
                    relinked: Some(Relinked {
                        link,
                        routed,
                        session: params.session,
                        config_changes,
                        settings,
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
                        carry(&link, &session, changes).await;
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
                                config_changes,
                                settings,
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

    /// A change the daemon refused, and why.
    struct Refused {
        change: SessionChange,
        error: ClientError,
    }

    /// Applies `changes` to `session` in order, returning its options
    /// after the last one that applied, or the first refusal.
    async fn configure_all(
        link: &Link,
        session: &SessionId,
        changes: Vec<SessionChange>,
    ) -> Result<Option<SessionOptions>, Box<Refused>> {
        let mut options = None;
        for change in changes {
            let params = SessionConfigureParams {
                session: session.clone(),
                change,
            };
            match link.request::<SessionConfigure>(&params).await {
                Ok(response) => options = Some(response.options),
                Err(error) => {
                    return Err(Box::new(Refused {
                        change: params.change,
                        error,
                    }))
                }
            }
        }
        Ok(options)
    }

    /// Carries `changes` to `session`; one the daemon refuses keeps its
    /// default.
    async fn carry(link: &Link, session: &SessionId, changes: Vec<SessionChange>) {
        for change in changes {
            if let Err(refused) = configure_all(link, session, vec![change]).await {
                tracing::warn!(error = %refused.error, "could not carry a setting to the new daemon session");
            }
        }
    }

    /// One daemon session the TUI follows.
    pub(crate) struct DaemonSession {
        /// Holds the connection; dropping it closes the connection, which
        /// detaches the session.
        daemon: Arc<Daemon>,
        id: SessionId,
        /// Whether the session has something to resume, so the exit
        /// receipt offers to: it was resumed, has a title, or took a
        /// prompt. One that keeps no file never has (see `keeps_file`).
        saved: bool,
        /// Whether the daemon saves the session: not one opened without
        /// saving, as `--no-save` opens them.
        keeps_file: bool,
        /// What `/new` and a resume from here open their session with.
        launch: DaemonLaunch,
        /// What opened the session, while it was opened new: a restarted
        /// daemon cannot reopen it until it saves, so it opens a new one.
        opened: Option<SessionNewParams>,
        workspace: PathBuf,
        routed: mpsc::UnboundedReceiver<Routed>,
        /// `rho/config/changed` for the settings last read, on the
        /// connection the session is attached on.
        config_changes: mpsc::UnboundedReceiver<ConfigChangedParams>,
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
        /// The events of the interactive login under way, if any.
        logins: Option<mpsc::UnboundedReceiver<LoginEvent>>,
    }

    impl DaemonSession {
        /// Opens or resumes `target` as `launch` says, attaches to it, and
        /// applies `settings`. The daemon starts when none runs.
        pub(crate) async fn open(
            launch: DaemonLaunch,
            target: DaemonTarget,
            settings: InitialSettings,
        ) -> anyhow::Result<Self> {
            let daemon = Arc::new(Daemon::for_frontend());
            let (id, opened) = match target {
                DaemonTarget::New => {
                    let params = SessionNewParams {
                        cwd: launch.cwd.clone(),
                        environment: ClientEnvironment::of_this_process(),
                        agent: launch.agent.clone(),
                        setup: launch.setup.clone(),
                        parent: None,
                        grant: None,
                    };
                    let created = daemon
                        .request::<SessionNew>(&params)
                        .await
                        .context("could not open a daemon session")?;
                    (created.session, Some(params))
                }
                DaemonTarget::Resume(session) => (session, None),
            };
            // Routed before the attach, so the replay that follows its
            // response is not dropped.
            let routed = route(&daemon, &id);
            let params = SessionAttachParams {
                session: id.clone(),
                after: None,
                environment: ClientEnvironment::of_this_process(),
                setup: launch.setup.clone(),
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
            // After the attach, which loads a saved session. The daemon
            // also sends each change's options on the session's stream.
            let at_rest = !matches!(
                response.state,
                JobState::Queued | JobState::Running | JobState::WaitingForInput
            );
            // A change waits for the turn under way, which may wait for
            // this client: those go through the request queue once the
            // session is followed, and a refusal shows as a row.
            let mut queued = Vec::new();
            let required = |changes: Vec<LaunchChange>| -> Vec<SessionChange> {
                changes
                    .into_iter()
                    .map(|change| change.of(&response.options))
                    .collect()
            };
            let changed = match settings {
                InitialSettings::Kept => None,
                InitialSettings::Required(changes) if !at_rest => {
                    queued = required(changes);
                    None
                }
                InitialSettings::Required(changes) => {
                    match configure_all(&link, &id, required(changes)).await {
                        Ok(options) => options,
                        Err(refused) => {
                            let Refused { change, error } = *refused;
                            daemon.unroute(&id);
                            return Err(anyhow::Error::new(error).context(format!(
                                "could not set the session's {}",
                                setting_name(&change)
                            )));
                        }
                    }
                }
                InitialSettings::Carried(carried) => {
                    carry(&link, &id, changes_between(&response.options, &carried)).await;
                    None
                }
            };
            // Routed before the reads, so no change after them is dropped.
            let config_changes = route_config(&daemon);
            let settings = load_settings(&link).await;
            let (target, requests) = watch::channel(Target::Attached {
                link,
                session: id.clone(),
            });
            let mut events = DaemonEvents::default();
            let pending = begin_replay(&mut events, &response);
            let mut session = Self {
                position: StreamPosition::attached(&response),
                daemon,
                id,
                saved: opened.is_none() || response.title.is_some(),
                keeps_file: !opened.as_ref().is_some_and(|opened| {
                    opened
                        .setup
                        .without
                        .contains(&rho_protocol::SessionPart::Saving)
                }),
                launch,
                opened,
                workspace: response.workspace,
                routed,
                config_changes,
                target,
                options: Some(changed.unwrap_or_else(|| response.options.clone())),
                events,
                pending,
                ended: false,
                requests: Some(DaemonRequests::new(requests)),
                reattach: None,
                logins: None,
            };
            session.pending.push(settings_loaded(settings));
            for change in queued {
                session
                    .requests
                    .as_ref()
                    .expect("attached session has requests")
                    .carry(change)?;
            }
            Ok(session)
        }

        /// Opens `target` on a connection of its own, to follow in this
        /// session's place: with this launch's setup, and a new session
        /// with the settings this one has now. This session keeps running
        /// in the daemon until the caller drops it, which detaches it.
        pub(in crate::tui) async fn open_sibling(
            &self,
            target: DaemonTarget,
        ) -> anyhow::Result<Self> {
            let settings = match (&target, &self.options) {
                (DaemonTarget::New, Some(options)) => {
                    InitialSettings::Carried(Box::new(options.clone()))
                }
                (DaemonTarget::New, None) | (DaemonTarget::Resume(_), _) => InitialSettings::Kept,
            };
            Self::open(self.launch.clone(), target, settings).await
        }

        /// The session's id, once a resume, saved title, or catalog lookup
        /// confirmed it has something to resume.
        pub(crate) fn saved_id(&self) -> Option<&str> {
            self.is_saved().then_some(self.id.as_str())
        }

        /// Whether the daemon has a file for the session.
        fn is_saved(&self) -> bool {
            self.keeps_file
                && (self.saved
                    || self
                        .requests
                        .as_ref()
                        .is_some_and(DaemonRequests::saved_rows))
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
            if let Some(mut views) = self.finish_reattach().await {
                self.take_settings_changes(&mut views);
                return views;
            }
            // Views already queued, such as the settings first read, go
            // out before anything that arrives later, so a change never
            // lands before the settings it changes. Taken only once the
            // wait below ends, so cancelling it loses nothing.
            if !self.pending.is_empty() {
                let mut views = std::mem::take(&mut self.pending);
                self.take_settings_changes(&mut views);
                return views;
            }
            let mut views = Vec::new();
            tokio::select! {
                biased;
                routed = self.routed.recv() => self.read(routed, &mut views),
                Some(error) = self.requests.as_mut().expect("attached session has requests").views.recv() => {
                    views.push(error);
                }
                Some(changed) = self.config_changes.recv() => {
                    views.push(DaemonView::SettingsChanged(Box::new(changed)));
                }
                Some(update) = auth::next_login(&mut self.logins) => {
                    views.push(DaemonView::Login(update));
                }
            }
            // A settings change that arrived meanwhile rides along, so a
            // streaming turn's events never hold it back.
            self.take_settings_changes(&mut views);
            views
        }

        /// Appends every settings change and login step already received,
        /// after `views`: neither waits for the session's events to pause.
        fn take_settings_changes(&mut self, views: &mut Vec<DaemonView>) {
            while let Ok(changed) = self.config_changes.try_recv() {
                views.push(DaemonView::SettingsChanged(Box::new(changed)));
            }
            views.extend(auth::take_logins(&mut self.logins).map(DaemonView::Login));
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
            self.take_settings_changes(&mut views);
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
            let mut lost_conversation = false;
            match reattached {
                Ok(Reattached { response, relinked }) => {
                    if let Some(Relinked {
                        link,
                        routed,
                        session,
                        config_changes,
                        settings,
                    }) = relinked
                    {
                        // A session with no file that took a prompt: the
                        // new session in its place has none of it.
                        lost_conversation = session != self.id && self.saved && !self.keeps_file;
                        self.config_changes = config_changes;
                        views.push(settings_loaded(settings));
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
                    if lost_conversation {
                        // After the replay, which clears the transcript.
                        views.push(DaemonView::Notice(
                            "the daemon restarted, and this unsaved session's conversation went with it; this is a new session".into(),
                        ));
                    }
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
                setup: self.launch.setup.clone(),
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
                                .filter(|_| !self.is_saved())
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
        fn end(&mut self, error: DaemonView) -> Vec<DaemonView> {
            self.ended = true;
            self.target.send_replace(Target::Ended);
            let mut views = vec![
                error,
                DaemonView::Turn(TurnState::Resting(Rest::Unfollowed)),
            ];
            // Its connection is gone, and nothing reads its events now.
            // What it already sent still counts: a login that finished
            // before the connection went was saved.
            if self.logins.is_some() {
                let queued: Vec<_> = auth::take_logins(&mut self.logins).collect();
                let finished = queued
                    .iter()
                    .any(|update| matches!(update, LoginUpdate::Finished { .. }));
                views.extend(queued.into_iter().map(DaemonView::Login));
                self.logins = None;
                self.daemon.unroute_login();
                if !finished {
                    views.push(DaemonView::Login(LoginUpdate::Lost));
                }
            }
            views
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

        /// The session's id, saved or not.
        pub(in crate::tui) fn id(&self) -> &str {
            self.id.as_str()
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
