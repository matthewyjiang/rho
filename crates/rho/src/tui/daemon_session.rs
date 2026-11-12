//! The daemon session the TUI follows (#1667; see `app/interactive.rs`).
//!
//! A new session does not exist in the daemon until its first prompt:
//! [`DaemonSession::draft`] only connects (`app::daemon_client`, which
//! starts the daemon when none runs), and shows the options the session
//! will open with, which the person may change meanwhile (`draft`). The
//! first prompt opens it with them and attaches, as does a command that reads
//! the live session (`/mcp`, `/context`), so opening the TUI, or `/new`,
//! and quitting leaves no session behind. Context a `!` command or
//! `/remember` adds meanwhile goes with the first prompt; quitting before
//! one drops it. [`DaemonSession::open`]
//! resumes a saved session and attaches to it: at launch before the TUI
//! takes the terminal, so a refusal reads as an ordinary startup error, and
//! again for each session the TUI switches to (`daemon_switch`). A daemon of
//! another build than the TUI shows its notice at launch and after each
//! reattach, not for a switch. The app loop then waits on
//! [`next_views`], which maps what the daemon sent since through
//! [`DaemonEvents`]: the reset replay of the saved conversation first, then
//! live events. Once the replay is over, the job state the attach answered
//! with says whether a turn runs, and the input requests still open show.
//!
//! The TUI prompts, cancels, steers, and withdraws steers through it
//! (`daemon_turn`), and answers and holds input requests (`daemon_answers`);
//! configures options through a background request queue. Once connected,
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

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use rho_protocol::{
    methods::SessionAttach, ClientEnvironment, ConfigChangedParams, ContentBlock, DetachReason,
    SessionAttachParams, SessionAttachResponse, SessionId, SessionNewParams, SessionOptions,
};
use tokio::sync::{mpsc, oneshot, watch};

use super::daemon_launch::{DaemonLaunch, DaemonTarget, InitialSettings};
use super::{
    daemon_events::{DaemonEvents, DaemonView, LoginUpdate, Rest, TurnState},
    daemon_requests::{DaemonRequests, Target},
    daemon_settings::DaemonSettings,
    daemon_settings_read::settings_loaded,
};
use crate::{
    app::{
        available_auths,
        daemon_client::{Daemon, Link, LoginEvent, Part, Routed, StreamPosition},
        DraftOptions,
    },
    credential_store::AppCredentialStore,
    daemon::DAEMON_RESTART_TIMEOUT,
};

mod attach;
mod auth;
mod draft;
mod initial_settings;
mod mcp;
mod relink;
mod requests;
mod tool_sources;
mod turn_requests;

use attach::{Attached, Connection};
use draft::Draft;
pub(in crate::tui) use mcp::DaemonMcp;
use relink::{Reattach, Reattached, Relink, Relinked};
use tool_sources::ToolSources;

const LOST: &str = "lost the connection to the rho daemon";

/// Why a request about the session waits for nothing before it opened.
const NOT_OPENED: &str = "the session opens with its first prompt";

/// Shown while the TUI reconnects to a restarting daemon.
const RECONNECTING: &str = "rho daemon restarting; reconnecting";

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

/// One daemon session the TUI follows.
pub(crate) struct DaemonSession {
    /// Holds the connection; dropping it closes the connection, which
    /// detaches the session.
    daemon: Arc<Daemon>,
    /// What `/new` and a resume from here open their session with.
    launch: DaemonLaunch,
    /// `rho/config/changed` for the settings last read, on the
    /// connection the session is attached on.
    config_changes: mpsc::UnboundedReceiver<ConfigChangedParams>,
    /// Where the session's requests go.
    target: watch::Sender<Target>,
    /// Views made outside the event stream, shown with the next views:
    /// the attach response's, such as the reset replay's
    /// [`DaemonView::Clear`], a draft's options, and a refused answer's or
    /// hold's.
    pending: Vec<DaemonView>,
    requests: Option<DaemonRequests>,
    /// The events of the interactive login under way, if any.
    logins: Option<mpsc::UnboundedReceiver<LoginEvent>>,
    following: Following,
}

/// Whether the session exists in the daemon yet.
enum Following {
    /// Not until the first prompt opens it with these settings.
    Draft(Box<Draft>),
    Attached(Box<Attachment>),
}

/// A session the daemon opened and the TUI attached to.
struct Attachment {
    id: SessionId,
    /// Whether the session has something to resume, so the exit
    /// receipt offers to: it was resumed, has a title, or took a
    /// prompt. One that keeps no file never has (see `keeps_file`).
    saved: bool,
    /// Whether the daemon saves the session: not one opened without
    /// saving, as `--no-save` opens them.
    keeps_file: bool,
    /// What opened the session, while it was opened new: a restarted
    /// daemon cannot reopen it until it saves, so it opens a new one.
    opened: Option<SessionNewParams>,
    workspace: PathBuf,
    routed: mpsc::UnboundedReceiver<Routed>,
    /// The session's options as the daemon last reported them.
    options: Option<SessionOptions>,
    events: DaemonEvents,
    /// Which of the attachment's events are its replay.
    position: StreamPosition,
    /// Set once the daemon ended the attachment; nothing more arrives.
    ended: bool,
    /// Kept across cancelled `next_views` waits; never restart an attach
    /// merely because the TUI handled a key or redrew.
    reattach: Option<Reattach>,
    /// Whether the session's tool sources still load.
    tool_sources: ToolSources,
    /// The one watch of them while they load, started by each attach:
    /// the first, and the one after a daemon restart. Only it says they
    /// have loaded.
    tool_source_watch: Option<tool_sources::Watch>,
}

impl Attachment {
    /// Whether the daemon has a file for the session; `requests` says
    /// whether a change saved rows to it.
    fn is_saved(&self, requests: Option<&DaemonRequests>) -> bool {
        self.keeps_file && (self.saved || requests.is_some_and(DaemonRequests::saved_rows))
    }
}

impl DaemonSession {
    /// Resumes saved session `id` as `launch` says, attaches to it, and
    /// applies `settings`, at launch. The daemon starts when none runs.
    /// A daemon of another build than this process shows its notice,
    /// once: a session the TUI switches to reaches the same daemon.
    pub(crate) async fn open(
        launch: DaemonLaunch,
        id: SessionId,
        settings: InitialSettings,
    ) -> anyhow::Result<Self> {
        let mut session = Self::follow(launch, id, settings).await?;
        session.show_skew().await;
        Ok(session)
    }

    /// Connects to the daemon, which starts when none runs, for a new
    /// session the first prompt opens with `options`, at launch. A daemon
    /// of another build shows its notice, as [`Self::open`] says.
    pub(crate) async fn draft(launch: DaemonLaunch, options: DraftOptions) -> anyhow::Result<Self> {
        let mut session = Self::drafted(launch, options).await?;
        session.show_skew().await;
        Ok(session)
    }

    /// Shows the notice of a daemon of another build than this process.
    async fn show_skew(&mut self) {
        let skew = self.daemon.current().await.and_then(|link| link.skew());
        if let Some(skew) = skew {
            self.pending.push(DaemonView::Notice(skew.to_string()));
        }
    }

    /// Resumes saved session `id` as `launch` says, on a connection of its
    /// own, attaches to it, and applies `settings`. The daemon starts when
    /// none runs.
    async fn follow(
        launch: DaemonLaunch,
        id: SessionId,
        settings: InitialSettings,
    ) -> anyhow::Result<Self> {
        let Connection {
            daemon,
            link: _,
            config_changes,
            settings: read,
        } = Connection::open().await?;
        let Attached {
            attachment,
            link,
            views,
            queued,
        } = Self::attach(&daemon, id, /*opened*/ None, &launch, settings).await?;
        let (target, requests) = watch::channel(Target::Attached {
            link,
            session: attachment.id.clone(),
        });
        let mut pending = views;
        pending.push(settings_loaded(read));
        let session = Self {
            requests: Some(DaemonRequests::new(requests, Arc::clone(&daemon))),
            daemon,
            launch,
            config_changes,
            target,
            pending,
            logins: None,
            following: Following::Attached(Box::new(attachment)),
        };
        for change in queued {
            session
                .requests
                .as_ref()
                .expect("attached session has requests")
                .carry(change)?;
        }
        Ok(session)
    }

    /// A session whose attachment has already ended, for tests that need an
    /// app but no daemon: it never connects, sends no views, and its
    /// requests fail.
    #[cfg(test)]
    pub(in crate::tui) fn ended_for_tests(cwd: PathBuf) -> Self {
        use rho_protocol::{JobState, Replay};

        let response = crate::app::daemon_client::test_support::attach_response(
            Replay::Reset,
            /*replay_events*/ 0,
            JobState::Idle,
            /*turn*/ None,
        );
        Self {
            daemon: Arc::new(Daemon::for_frontend()),
            launch: DaemonLaunch {
                cwd: cwd.clone(),
                agent: None,
                setup: rho_protocol::SessionSetup::default(),
                config: Arc::default(),
            },
            config_changes: mpsc::unbounded_channel().1,
            target: watch::channel(Target::Ended).0,
            pending: Vec::new(),
            requests: None,
            logins: None,
            following: Following::Attached(Box::new(Attachment {
                position: StreamPosition::attached(&response),
                id: SessionId::new("test-session"),
                saved: false,
                keeps_file: true,
                opened: None,
                workspace: cwd,
                routed: mpsc::unbounded_channel().1,
                options: None,
                events: DaemonEvents::default(),
                ended: true,
                reattach: None,
                tool_sources: ToolSources::Loaded,
                tool_source_watch: None,
            })),
        }
    }

    /// This session, as one the daemon has saved.
    #[cfg(test)]
    pub(in crate::tui) fn saved_for_tests(mut self) -> Self {
        if let Following::Attached(attachment) = &mut self.following {
            attachment.saved = true;
        }
        self
    }

    /// Follows `target` in this session's place, on a connection of its
    /// own: with this launch's setup, and a new session with the settings
    /// this one has now. This session keeps running in the daemon until
    /// the caller drops it, which detaches it.
    pub(in crate::tui) async fn open_sibling(&self, target: DaemonTarget) -> anyhow::Result<Self> {
        match target {
            DaemonTarget::New => {
                let options = match &self.following {
                    Following::Draft(draft) => draft.options.clone(),
                    Following::Attached(attachment) => {
                        let config = (*self.launch.config).clone();
                        let auths = available_auths(Arc::new(AppCredentialStore)).await;
                        let added_dirs = self.launch.setup.additional_directories.clone();
                        match &attachment.options {
                            Some(options) => {
                                DraftOptions::carried(options, config, auths, added_dirs)
                            }
                            None => DraftOptions::new(config, auths, Vec::new(), added_dirs),
                        }
                    }
                };
                Self::drafted(self.launch.clone(), options).await
            }
            DaemonTarget::Resume(id) => {
                Self::follow(self.launch.clone(), id, InitialSettings::Kept).await
            }
        }
    }

    /// The followed session's attachment, once the daemon opened it.
    fn attachment(&self) -> Option<&Attachment> {
        match &self.following {
            Following::Attached(attachment) => Some(attachment),
            Following::Draft(_) => None,
        }
    }

    /// The session's id, once a resume, saved title, or catalog lookup
    /// confirmed it has something to resume.
    pub(crate) fn saved_id(&self) -> Option<&str> {
        self.attachment()
            .filter(|attachment| attachment.is_saved(self.requests.as_ref()))
            .map(|attachment| attachment.id.as_str())
    }

    /// What this launch opens its sessions with.
    pub(in crate::tui) fn launch(&self) -> &DaemonLaunch {
        &self.launch
    }

    /// The daemon the session runs in, for requests about its other
    /// sessions and jobs.
    pub(in crate::tui) fn daemon(&self) -> Arc<Daemon> {
        Arc::clone(&self.daemon)
    }

    /// The directory the session works in, as the daemon reports it, or
    /// the launch's before the session opened.
    pub(crate) fn workspace(&self) -> &Path {
        self.attachment()
            .map_or(&self.launch.cwd, |attachment| &attachment.workspace)
    }

    /// Whether the followed attachment ended; one not opened yet never has.
    fn ended(&self) -> bool {
        self.attachment().is_some_and(|attachment| attachment.ended)
    }

    /// What the daemon sent since the last call, mapped; waits for a
    /// notification or reattach response. Queued replay events read as
    /// one batch rather than one frame per event.
    ///
    /// Cancel-safe: the reattach future stays in the session, and no
    /// queued views are taken until the wait finishes.
    pub(super) async fn next_views(&mut self) -> Vec<DaemonView> {
        if self.ended() {
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
            next = next_of(&mut self.following) => match next {
                Next::Routed(routed) => self.read(routed, &mut views),
                Next::ToolSourcesLoaded => views.push(DaemonView::ToolSourcesLoaded),
            },
            Some(error) = self.requests.as_mut().expect("followed session has requests").views.recv() => {
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
        let first = match &mut self.following {
            Following::Attached(attachment)
                if !attachment.ended && attachment.reattach.is_none() =>
            {
                attachment.routed.try_recv().ok()
            }
            Following::Attached(_) | Following::Draft(_) => None,
        };
        if let Some(first) = first {
            self.read(Some(first), &mut views);
        }
        views
    }

    /// Waits for the reattach under way, if any, and takes over the
    /// attachment it came back with, or ends following when it failed;
    /// the views to show, `pending` first.
    async fn finish_reattach(&mut self) -> Option<Vec<DaemonView>> {
        let Following::Attached(attachment) = &mut self.following else {
            return None;
        };
        let reattached = attachment.reattach.as_mut()?.await;
        attachment.reattach = None;
        let mut views = std::mem::take(&mut self.pending);
        let mut lost_conversation = false;
        let mut skew = None;
        match reattached {
            Ok(Reattached { response, relinked }) => {
                if let Some(Relinked {
                    link,
                    routed,
                    session,
                    config_changes,
                    settings,
                    tool_sources,
                }) = relinked
                {
                    // A session with no file that took a prompt: the
                    // new session in its place has none of it.
                    lost_conversation =
                        session != attachment.id && attachment.saved && !attachment.keeps_file;
                    self.config_changes = config_changes;
                    views.push(settings_loaded(settings));
                    let status = format!("reconnected to rho daemon {}", link.daemon().version);
                    views.push(DaemonView::Status(if session == attachment.id {
                        status
                    } else {
                        // It had saved nothing the restarted daemon could reopen.
                        format!("{status} (new session)")
                    }));
                    attachment.id = session.clone();
                    attachment.routed = routed;
                    // A prompt waits again while the restarted daemon
                    // loads the session's tool sources again.
                    attachment.tool_source_watch =
                        tool_sources::Watch::of(tool_sources, &link, &session);
                    if attachment.tool_sources == ToolSources::Loading
                        && tool_sources == ToolSources::Loaded
                    {
                        views.push(DaemonView::ToolSourcesLoaded);
                    }
                    attachment.tool_sources = tool_sources;
                    skew = link.skew();
                    self.target.send_replace(Target::Attached { link, session });
                }
                attachment.position.reattached(&response);
                attachment.options = Some(response.options.clone());
                attachment.saved |= response.title.is_some();
                attachment.workspace = response.workspace.clone();
                views.extend(begin_replay(&mut attachment.events, &response));
                // After the replay, which clears the transcript.
                if lost_conversation {
                    views.push(DaemonView::Notice(
                            "the daemon restarted, and this unsaved session's conversation went with it; this is a new session".into(),
                        ));
                }
                // The restarted daemon may still be another build.
                if let Some(skew) = skew {
                    views.push(DaemonView::Notice(skew.to_string()));
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

    /// Maps `first` and everything queued behind it into `views`;
    /// `None` is a closed route.
    fn read(&mut self, first: Option<Routed>, views: &mut Vec<DaemonView>) {
        let mut next = first;
        loop {
            let Following::Attached(attachment) = &mut self.following else {
                return;
            };
            match next {
                Some(Routed::Event { seq, event }) => {
                    let part = attachment.position.observe(seq);
                    match &event {
                        rho_protocol::SessionEvent::TitleChanged { .. } => attachment.saved = true,
                        rho_protocol::SessionEvent::OptionsChanged { options } => {
                            attachment.options = Some((**options).clone());
                        }
                        _ => {}
                    }
                    views.extend(attachment.events.map(event));
                    if let Part::Replayed { left: 0, .. } = part {
                        views.extend(attachment.events.end_replay());
                    }
                }
                Some(Routed::Detached(DetachReason::Lagged)) => {
                    let daemon = Arc::clone(&self.daemon);
                    let params = reattach_params(attachment, &self.launch);
                    attachment.reattach = Some(Box::pin(async move {
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
                    // Its connection closes next, and a read that fails
                    // counts as loaded: the reattach reads them again.
                    attachment.tool_source_watch = None;
                    // The connection the session is attached on, taken
                    // now: the daemon closes it next.
                    let old = match self.target.send_replace(Target::Reconnecting) {
                        Target::Attached { link, .. } => Some(link),
                        Target::Unopened | Target::Reconnecting | Target::Ended => None,
                    };
                    let relink = Relink {
                        daemon: Arc::clone(&self.daemon),
                        params: reattach_params(attachment, &self.launch),
                        // Only a session that saved nothing can be
                        // missing from the restarted daemon.
                        opened: attachment
                            .opened
                            .clone()
                            .filter(|_| !attachment.is_saved(self.requests.as_ref()))
                            .map(|opened| (opened, attachment.options.clone())),
                    };
                    attachment.reattach = Some(Box::pin(async move {
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
            match attachment.routed.try_recv() {
                Ok(routed) => next = Some(routed),
                // Empty, or closed, which the next call reads as lost.
                Err(_) => return,
            }
        }
    }

    /// The attachment is over: `error` says why, and the TUI stops
    /// showing a turn it can no longer follow.
    fn end(&mut self, error: DaemonView) -> Vec<DaemonView> {
        if let Following::Attached(attachment) = &mut self.following {
            attachment.ended = true;
        }
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
            Target::Unopened => anyhow::bail!(NOT_OPENED),
            Target::Attached { .. } | Target::Reconnecting | Target::Ended => {
                anyhow::bail!(LOST)
            }
        }
    }

    /// The connection requests about the daemon rather than the session
    /// go on: the attached one, or before the session opened, the
    /// daemon's, connecting again when it closed.
    async fn connected_link(&self) -> anyhow::Result<Arc<Link>> {
        if matches!(*self.target.borrow(), Target::Unopened) {
            return Ok(self.daemon.link().await?);
        }
        self.attached_link().await
    }

    /// Starts a turn of `content` in the opened session. Once this returns, the views of what the daemon
    /// sent before it answered, such as the turn's start, are waiting for
    /// [`Self::take_views`], and the session is saved: the daemon answers
    /// once the turn saved its prompt.
    pub(in crate::tui) async fn prompt(
        &mut self,
        content: Vec<ContentBlock>,
        author: rho_protocol::PromptAuthor,
    ) -> anyhow::Result<()> {
        // The app opens a new session first (`open_daemon_draft`), so a
        // failure reads as the open's own.
        if self.is_draft() {
            anyhow::bail!(NOT_OPENED);
        }
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
            .prompt(content, author)?;
        // The requests queue sends it under one idempotency key, again
        // on the restarted daemon when this one refused it as stopping,
        // which may be after its turn already saved the prompt: the
        // daemon runs it once.
        self.await_answer(answer).await?;
        if let Following::Attached(attachment) = &mut self.following {
            attachment.saved = true;
        }
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
            let Following::Attached(attachment) = &mut self.following else {
                return answer.await.unwrap_or_else(|_| Err(anyhow::anyhow!(LOST)));
            };
            if attachment.ended {
                return answer.await.unwrap_or_else(|_| Err(anyhow::anyhow!(LOST)));
            }
            if attachment.reattach.is_some() {
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
                    routed = attachment.routed.recv() => {
                        let mut views = Vec::new();
                        self.read(routed, &mut views);
                        self.pending.extend(views);
                    }
                }
            }
        }
    }

    /// The session's id, saved or not, once the daemon opened it.
    pub(in crate::tui) fn id(&self) -> Option<&str> {
        self.attachment().map(|attachment| attachment.id.as_str())
    }

    /// The opened session's id, for requests that need one: the turn's
    /// and its input requests', which only an opened session has.
    fn opened_id(&self) -> anyhow::Result<SessionId> {
        self.attachment()
            .map(|attachment| attachment.id.clone())
            .context(NOT_OPENED)
    }
}

/// The params that attach `attachment` again after its cursor, with
/// `launch`'s setup.
fn reattach_params(attachment: &Attachment, launch: &DaemonLaunch) -> SessionAttachParams {
    SessionAttachParams {
        session: attachment.id.clone(),
        after: attachment.position.cursor().cloned(),
        environment: ClientEnvironment::of_this_process(),
        setup: launch.setup.clone(),
    }
}

/// What the followed session's attachment brought next.
enum Next {
    /// What the router handed it; `None` is a closed route.
    Routed(Option<Routed>),
    /// Its tool sources loaded.
    ToolSourcesLoaded,
}

/// The next of what the followed session's attachment brings: what the
/// router hands it, or its tool sources loading. Never before the session
/// opened. Cancel-safe.
async fn next_of(following: &mut Following) -> Next {
    let Following::Attached(attachment) = following else {
        return std::future::pending().await;
    };
    tokio::select! {
        biased;
        routed = attachment.routed.recv() => Next::Routed(routed),
        () = tool_sources::loaded(&mut attachment.tool_source_watch) => {
            attachment.tool_sources = ToolSources::Loaded;
            Next::ToolSourcesLoaded
        }
    }
}

/// A tail keeps message/card state, but open prompts are rebuilt from
/// the attach snapshot and replay, just as for a reset. Returns the
/// views the response makes.
fn begin_replay(events: &mut DaemonEvents, response: &SessionAttachResponse) -> Vec<DaemonView> {
    let mut views = vec![events.replay(response.replay)];
    views.push(DaemonView::Options(Box::new(response.options.clone())));
    if let Some(title) = &response.title {
        views.push(DaemonView::Title(title.clone()));
    }
    let turn_since = response
        .turn_elapsed_ms
        .and_then(|elapsed| Instant::now().checked_sub(Duration::from_millis(elapsed)));
    events.attached(
        response.inputs.clone(),
        response.state,
        response.turn.clone(),
        turn_since,
    );
    if response.replay_events == 0 {
        views.extend(events.end_replay());
    }
    views
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
