//! The daemon's pull request service as its sessions see it: one
//! [`PullRequests`] for the whole daemon, whose poller reads every watched
//! pull request with one GitHub budget, and the inbox each daemon root
//! session's tools and delivery turns use (see `crate::app::pull_requests`).
//!
//! A loaded session's idle watcher hears its inbox (see `idle_wake`). An
//! unloaded one has no watcher, so the idle unload keeps the opening of a
//! session that watches a pull request and whose client asked for daemon
//! turns (`idle_wake: turn`): the environment and grant of its last prompt
//! and the setup it opened with. News reopens it with that, as a request
//! would: adding it starts its idle watcher, which finds the news and
//! starts the delivery turn.
//!
//! A batch of news reopens a session once: each reopen for news records
//! the session's news generation it was for, and only news queued since
//! reopens it again. So a refused reopen, or a delivery turn that fails and
//! lets the session unload again with the news, leaves that news for the
//! session's next turn of any origin rather than loop.
//!
//! An opening goes once used, when any request reopens the session, when
//! its last watch ended, when the session is deleted, and when the daemon
//! stops. It holds the client's environment, credentials included, so it
//! lives only here, in memory, is never logged, and has no `Debug`. After a
//! daemon restart there is none: news waits for the next attach.
//!
//! A session's pull requests go when it is deleted: at once when the
//! daemon deletes it, and otherwise at the daemon's next check for sessions
//! it has not loaded and nothing is saved under (see
//! [`Sessions::forget_deleted_pull_requests`]).

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use rho_sdk::SessionId;

use super::{open::OpenWith, store::store_blocking, LoadedSession, Sessions};
#[cfg(test)]
use crate::app::pull_requests::ScriptedForge;
use crate::{
    app::{
        daemon_host::wire::session_to_wire,
        pull_requests::{unix_ms, Forge, PullRequestInbox, PullRequests, WATCH_POLL},
    },
    session::Session as StoredSession,
};

/// The daemon's pull request service, owned by [`Sessions`], and what it
/// keeps to wake the unloaded sessions its news is for.
pub(super) struct PullRequestWake {
    service: PullRequests,
    /// By session; see [`Kept`].
    kept: Mutex<HashMap<SessionId, Kept>>,
}

/// What the daemon keeps to reopen one unloaded session for news.
#[derive(Default)]
struct Kept {
    /// The opening the idle unload kept, until a reopen uses it or it goes.
    opening: Option<OpenWith>,
    /// The news generation (see [`PullRequests::news_generation`]) the last
    /// reopen for news was for: only news queued since reopens it again.
    reopened_for: u64,
}

impl PullRequestWake {
    /// Opens the service over the records in the daemon's state dir under
    /// `rho_home`, reading pull requests through `forge`.
    pub(super) fn open(rho_home: &Path, forge: Arc<dyn Forge>) -> Self {
        Self {
            service: PullRequests::open(&rho_home.join("daemon"), forge),
            kept: Mutex::default(),
        }
    }

    /// The inbox of session `id`, which works in `cwd`.
    pub(super) fn inbox(&self, id: &SessionId, cwd: &Path) -> PullRequestInbox {
        self.service.inbox(id, cwd)
    }

    /// The service itself, which clients list and show sessions' pull
    /// requests through (see `pull_requests`).
    pub(super) fn service(&self) -> &PullRequests {
        &self.service
    }

    /// Session `id` was deleted: its links, watches, news and kept opening
    /// go with it.
    pub(super) fn forget(&self, id: &SessionId) {
        self.kept().remove(id);
        self.service.forget(id);
    }

    /// Drops the opening the idle unload kept for session `id`, if any.
    pub(super) fn drop_opening(&self, id: &SessionId) {
        if let Some(kept) = self.kept().get_mut(id) {
            kept.opening = None;
        }
    }

    /// The opening to reopen session `id` with, taken, when it has news no
    /// reopen was for yet. The opening of a session that has no news and
    /// watches nothing goes: no news can come to use it.
    fn opening_for_news(&self, id: &SessionId) -> Option<OpenWith> {
        let service = &self.service;
        let (news, watching) = (service.has_news(id), service.watches(id));
        let generation = service.news_generation(id);
        let mut kept = self.kept();
        let kept = kept.get_mut(id)?;
        match (news, watching) {
            (true, _) if generation > kept.reopened_for => {
                let opening = kept.opening.take()?;
                kept.reopened_for = generation;
                Some(opening)
            }
            (true, _) | (false, true) => None,
            (false, false) => {
                kept.opening = None;
                None
            }
        }
    }

    fn kept(&self) -> MutexGuard<'_, HashMap<SessionId, Kept>> {
        // Each change is one insert, field write, removal or clear.
        self.kept.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Sessions {
    /// Polls the pull requests that are linked to sessions or that clients
    /// look at, wakes the unloaded sessions that get news, marks done the
    /// sessions whose linked pull requests have all merged (see `marks`),
    /// tells loaded sessions' clients their pull requests as they change,
    /// and forgets those of sessions deleted behind the daemon's back,
    /// until the daemon starts stopping; then drops every kept opening.
    pub(in crate::app::daemon_host) async fn poll_pull_requests(&self) {
        tokio::join!(
            self.pull_requests.service.run(self.stopping.cancelled()),
            self.wake_unloaded(),
            self.mark_merged_done(),
            self.report_pull_requests(unix_ms),
            self.forget_deleted_sessions(),
        );
        self.pull_requests.kept().clear();
    }

    /// Runs [`Self::forget_deleted_pull_requests`] at once, which at the
    /// daemon's start is before the poll loop's first period ends, and then
    /// every [`WATCH_POLL`], until the daemon starts stopping. As often as a
    /// watch is read, so a deleted session's watches are read about once
    /// more at most.
    async fn forget_deleted_sessions(&self) {
        loop {
            self.forget_deleted_pull_requests().await;
            tokio::select! {
                () = self.stopping.cancelled() => return,
                () = tokio::time::sleep(WATCH_POLL) => {}
            }
        }
    }

    /// Forgets the pull requests of each session the daemon has not loaded
    /// and nothing is saved under: one another process deleted, such as
    /// `rho sessions rm` or the terminal UI's session manager, which refuse
    /// only a session the daemon has loaded; a session deleted with its
    /// parent; or one that had saved nothing when an earlier daemon
    /// stopped. Otherwise its watches and links would be read, and its news
    /// would try to wake it, for good.
    ///
    /// A loaded session keeps them though it saved nothing yet, as a new
    /// one a client linked a pull request to before its first prompt. One
    /// not loaded has saved, else the idle unload discarded it with its
    /// pull requests, and a session gone from the store stays gone: ids
    /// are never reused, so no reopen lock is needed.
    async fn forget_deleted_pull_requests(&self) {
        let mut unloaded = self.pull_requests.service.sessions();
        {
            let loaded = self.loaded();
            unloaded.retain(|id| !loaded.contains_key(id));
        }
        if unloaded.is_empty() {
            return;
        }
        let ids: Vec<String> = unloaded.iter().map(|id| id.as_str().to_owned()).collect();
        let rho_home = self.rho_home.clone();
        let saved = store_blocking("look up the sessions that have pull requests", move || {
            Ok(StoredSession::workspaces_of(&rho_home, &ids))
        })
        .await;
        let saved = match saved {
            Ok(saved) => saved,
            Err(error) => {
                tracing::warn!(%error, "could not check for deleted sessions that have pull requests");
                return;
            }
        };
        for (id, saved) in unloaded.iter().zip(saved) {
            match saved {
                Ok(Some(_)) => {}
                Ok(None) => {
                    tracing::info!(session = %id, "forgot the pull requests of a deleted session");
                    self.pull_requests.forget(id);
                }
                // Kept until a later check can tell.
                Err(error) => tracing::debug!(
                    session = %id,
                    error = format!("{error:#}"),
                    "could not tell whether a session that has pull requests is saved"
                ),
            }
        }
    }

    /// Wakes each session a wake request names, one at a time, until the
    /// daemon starts stopping.
    async fn wake_unloaded(&self) {
        let Some(mut requests) = self.pull_requests.service.take_wake_requests() else {
            return;
        };
        loop {
            let id = tokio::select! {
                () = self.stopping.cancelled() => return,
                Some(id) = requests.recv() => id,
                else => return,
            };
            self.wake_for_pull_requests(&id).await;
        }
    }

    /// Keeps the opening of session `id`, which the idle unload just took
    /// as `session`, when its client asked for daemon turns and it watches a
    /// pull request, so news can reopen it. Call with `id`'s reopen lock
    /// held, so no reopen runs first. News that came since the unload took
    /// the session found nothing to reopen, so this asks for its wake again,
    /// unless a reopen was for that news already.
    pub(super) fn retain_opening(&self, id: &SessionId, session: &LoadedSession) {
        let service = &self.pull_requests.service;
        if session.idle_wake.is_none() || !service.watches(id) {
            return;
        }
        // Its last prompt's, as a delivery turn takes it.
        let Some(grant) = self.jobs.grant_of(&session.job) else {
            return;
        };
        let opening = OpenWith {
            environment: session.environment.current(),
            setup: session.setup.clone(),
            grant,
        };
        let reopened_for = {
            let mut kept = self.pull_requests.kept();
            let kept = kept.entry(id.clone()).or_default();
            kept.opening = Some(opening);
            kept.reopened_for
        };
        // The stop drops every opening once it started; one kept since
        // goes here.
        if self.stopping.is_cancelled() {
            self.pull_requests.drop_opening(id);
            return;
        }
        if service.has_news(id) && service.news_generation(id) > reopened_for {
            service.request_wake(id);
        }
    }

    /// Wakes session `id`, which got pull request news: an unloaded one
    /// reopens with the opening the idle unload kept, once per batch of
    /// news. A loaded one's idle watcher hears the news itself.
    async fn wake_for_pull_requests(&self, id: &SessionId) {
        if self.loaded().contains_key(id) {
            return;
        }
        let Some(opening) = self.pull_requests.opening_for_news(id) else {
            return;
        };
        match self.session(&session_to_wire(id.clone()), &opening).await {
            Ok(_) => {
                tracing::info!(session = %id, "reopened an unloaded session for its pull request news");
            }
            Err(error) => {
                tracing::warn!(
                    session = %id,
                    %error,
                    "could not reopen an unloaded session for its pull request news; it waits for the session's next turn"
                );
            }
        }
    }
}

/// The pull request session tests watch.
#[cfg(test)]
pub(super) const TEST_PULL_REQUEST: &str = "acme/widgets#12";

/// When session tests watch [`TEST_PULL_REQUEST`], in unix milliseconds.
#[cfg(test)]
const TEST_WATCHED_AT: u64 = 1_700_000_000_000;

/// Has `inbox` watch [`TEST_PULL_REQUEST`] on `forge`, its checks `ci`
/// and `lint` running.
#[cfg(test)]
pub(super) async fn watch_for_tests(forge: &ScriptedForge, inbox: &PullRequestInbox) {
    forge.set_open(TEST_PULL_REQUEST, &["ci", "lint"], &[]);
    inbox
        .watch(TEST_PULL_REQUEST, TEST_WATCHED_AT)
        .await
        .expect("the scripted pull request can be watched");
}

#[cfg(test)]
impl PullRequestWake {
    /// Checks `failed` of [`TEST_PULL_REQUEST`] on `forge` failed, the
    /// others run still, as the poll `minutes` after the watch reads them.
    pub(super) async fn fail_checks_for_tests(
        &self,
        forge: &ScriptedForge,
        failed: &[&str],
        minutes: u64,
    ) {
        let running: Vec<&str> = ["ci", "lint"]
            .into_iter()
            .filter(|check| !failed.contains(check))
            .collect();
        forge.set_open(TEST_PULL_REQUEST, &running, failed);
        self.service.tick(TEST_WATCHED_AT + minutes * 60_000).await;
    }

    /// [`TEST_PULL_REQUEST`] merged on `forge`, as the poll `minutes` after
    /// the watch reads it.
    pub(super) async fn merge_for_tests(&self, forge: &ScriptedForge, minutes: u64) {
        forge.set_merged(TEST_PULL_REQUEST);
        self.service.tick(TEST_WATCHED_AT + minutes * 60_000).await;
    }

    /// Whether the idle unload's opening of session `id` is kept.
    pub(super) fn keeps_opening(&self, id: &SessionId) -> bool {
        self.kept()
            .get(id)
            .is_some_and(|kept| kept.opening.is_some())
    }

    /// The daemon's wake requests, the poller's and its own, which no
    /// waker takes in a test.
    pub(super) fn wake_requests_for_tests(
        &self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<SessionId> {
        self.service
            .take_wake_requests()
            .expect("no waker took the wake requests")
    }
}

#[cfg(test)]
#[path = "pull_request_wake_tests.rs"]
mod tests;
