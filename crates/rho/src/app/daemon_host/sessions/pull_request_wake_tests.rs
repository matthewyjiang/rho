use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_protocol::{DetachReason, SessionSetup};
use rho_sdk::SessionId;
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{watch_for_tests, TEST_PULL_REQUEST};
use crate::{
    app::{
        daemon_host::{
            jobs::{JobRegistry, JobStore, TurnRecord},
            sessions::{
                open::OpenWith,
                test_sessions::{load, Load},
                LoadedSession, Sessions,
            },
            wire::session_to_wire,
        },
        pull_requests::{PullRequestInbox, ScriptedForge},
    },
    caller_context::CallerContext,
};

/// A daemon whose pull requests `forge` scripts, with a session that
/// watches [`TEST_PULL_REQUEST`], its client asking for daemon turns.
struct Watching {
    _root: TempDir,
    _home: TempDir,
    forge: Arc<ScriptedForge>,
    sessions: Arc<Sessions>,
    session: Arc<LoadedSession>,
    inbox: PullRequestInbox,
    /// The daemon's wake requests, the poller's and its own; no waker runs.
    wakes: UnboundedReceiver<SessionId>,
}

/// What the daemon keeps of the unloaded session.
#[derive(Debug, PartialEq)]
struct Observed {
    /// Whether it keeps the session's opening.
    opening: bool,
    /// Whether anything asked to wake the session since the last look.
    asked: bool,
    /// Whether news waits for the session.
    news: bool,
}

impl Watching {
    async fn new() -> Self {
        let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let forge = ScriptedForge::new();
        let sessions = Sessions::new(
            Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
            home.path(),
            forge.clone(),
        );
        let session = load(&sessions, &root, Load::Waking).await;
        let inbox = sessions.pull_requests.inbox(&session.id, root.path());
        watch_for_tests(&forge, &inbox).await;
        let wakes = sessions.pull_requests.wake_requests_for_tests();
        Self {
            _root: root,
            _home: home,
            forge,
            sessions,
            session,
            inbox,
            wakes,
        }
    }

    fn id(&self) -> &SessionId {
        &self.session.id
    }

    /// As the idle unload takes the session, under its reopen lock.
    fn unload(&self) {
        self.sessions.loaded().remove(self.id());
        self.sessions.retain_opening(self.id(), &self.session);
    }

    /// Checks `failed` fail, as the poll `minutes` after the watch reads
    /// them: news for the session.
    async fn news(&self, failed: &[&str], minutes: u64) {
        self.sessions
            .pull_requests
            .fail_checks_for_tests(&self.forge, failed, minutes)
            .await;
    }

    /// As the daemon's waker does for a wake request.
    async fn wake(&self) {
        self.sessions.wake_for_pull_requests(self.id()).await;
    }

    /// What the daemon keeps of the session now.
    fn observed(&mut self) -> Observed {
        let mut asked = false;
        while let Ok(id) = self.wakes.try_recv() {
            asked |= id == self.session.id;
        }
        Observed {
            opening: self.sessions.pull_requests.keeps_opening(self.id()),
            asked,
            news: self.inbox.has_news(),
        }
    }

    /// The daemon begins stopping, which refuses every reopen.
    fn stop(&self) {
        self.sessions.begin_stop(DetachReason::DaemonStopping);
    }
}

/// What befalls a session that watches a pull request, its client asking
/// for daemon turns, once the idle unload took it and kept its opening.
#[derive(Clone, Copy, Debug)]
enum Case {
    /// A wake request comes, with no news.
    QuietWake,
    /// Its last watch ends, then a wake request comes with no news.
    UnwatchedWake,
    /// News comes, and its wake reopens it, which the stopping daemon
    /// refuses.
    NewsWake,
    /// News came while the unload took it, before it kept the opening.
    NewsDuringUnload,
    /// A request reopens it, which the stopping daemon refuses.
    Reopened,
    /// It is deleted.
    Deleted,
}

// Covers: an opening the idle unload kept waits while a quiet watch may
// still bring news, and goes once the last watch ended, once a reopen for
// news used it, refused or not, when any request reopens the session, and
// when it is deleted; news that came during the unload asks for a wake.
// A kept opening holds the client's credentials, so one kept past its use
// is a leak; a missed wake leaves the news unread.
// Owner: daemon pull request wake of unloaded sessions.
#[tokio::test]
async fn a_kept_opening_goes_once_nothing_needs_it() {
    let observed = |opening, asked, news| Observed {
        opening,
        asked,
        news,
    };
    let cases = [
        (Case::QuietWake, observed(true, false, false)),
        (Case::UnwatchedWake, observed(false, false, false)),
        (Case::NewsWake, observed(false, false, true)),
        (Case::NewsDuringUnload, observed(true, true, true)),
        (Case::Reopened, observed(false, false, false)),
        (Case::Deleted, observed(false, false, false)),
    ];
    for (case, expected) in cases {
        let mut watching = Watching::new().await;
        if let Case::NewsDuringUnload = case {
            watching.news(&["ci"], 1).await;
            // The poller's.
            watching.observed();
        }
        watching.unload();
        match case {
            Case::QuietWake => watching.wake().await,
            Case::NewsDuringUnload => {}
            Case::UnwatchedWake => {
                watching.inbox.unwatch(TEST_PULL_REQUEST).await.unwrap();
                watching.wake().await;
            }
            Case::NewsWake => {
                watching.news(&["ci"], 1).await;
                // The poller's.
                watching.observed();
                watching.stop();
                watching.wake().await;
            }
            Case::Reopened => {
                watching.stop();
                let session = &watching.session;
                let with = OpenWith {
                    environment: session.environment.current(),
                    setup: SessionSetup::default(),
                    grant: watching.sessions.jobs.grant_of(&session.job).unwrap(),
                };
                let id = session_to_wire(session.id.clone());
                let refused = watching.sessions.session(&id, &with).await;
                assert!(refused.is_err(), "a stopping daemon reopens nothing");
            }
            Case::Deleted => watching.sessions.pull_requests.forget(watching.id()),
        }
        assert_eq!(watching.observed(), expected, "{case:?}");
    }
}

// Covers: one batch of news reopens an unloaded session once. When the
// session unloads again with that news still waiting, as after a reopen
// whose delivery turn failed before it took the news, the unload keeps its
// opening but neither it nor a wake request reopens the session for that
// news; news queued since does. Otherwise a delivery turn that keeps
// failing reopens and unloads the session in a loop.
// Owner: daemon pull request wake of unloaded sessions.
#[tokio::test]
async fn one_batch_of_news_reopens_an_unloaded_session_once() {
    let mut watching = Watching::new().await;
    // A turn of another job writes the session, so the daemon refuses each
    // reopen before it looks for the session's file, which no test daemon
    // finds: a reopen for news is attempted, and uses the opening, all the
    // same.
    let session = &watching.session;
    watching
        .sessions
        .jobs
        .start_turn(
            &session.job,
            TurnRecord::starting(/*key*/ None),
            CallerContext::ROOT,
        )
        .unwrap();
    watching.unload();
    watching.news(&["ci"], 1).await;
    let asked_for_news = watching.observed();
    watching.wake().await;
    let reopened = watching.observed();
    watching.unload();
    let unloaded_again = watching.observed();
    watching.wake().await;
    let woken_again = watching.observed();
    watching.news(&["ci", "lint"], 2).await;
    let asked_for_newer_news = watching.observed();
    watching.wake().await;
    let reopened_for_newer_news = watching.observed();

    let observed = |opening, asked| Observed {
        opening,
        asked,
        news: true,
    };
    assert_eq!(
        [
            asked_for_news,
            reopened,
            unloaded_again,
            woken_again,
            asked_for_newer_news,
            reopened_for_newer_news,
        ],
        [
            observed(true, true),
            observed(false, false),
            observed(true, false),
            observed(true, false),
            observed(true, true),
            observed(false, false),
        ]
    );
}

// Covers: the daemon forgets the pull requests of a session it has not
// loaded and nothing is saved under, as after `rho sessions rm` deleted it
// while unloaded, and keeps those of a saved session it unloaded and of a
// loaded one that has saved nothing yet, which a client may just have
// linked one to. Otherwise a deleted session's watches are read for good,
// or a new session loses its pull requests.
// Owner: daemon pull request wake of unloaded sessions.
#[tokio::test]
async fn a_session_deleted_behind_the_daemon_loses_its_pull_requests() {
    let home = TempDir::new().unwrap();
    // The home's `sessions` directory, where the daemon finds saved sessions.
    let saved_root = tempfile::Builder::new()
        .prefix("sessions")
        .rand_bytes(0)
        .tempdir_in(home.path())
        .unwrap();
    // Outside the home: the daemon finds nothing saved there, as after a
    // delete.
    let deleted_root = TempDir::new().unwrap();
    let forge = ScriptedForge::new();
    let sessions = Sessions::new(
        Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
        home.path(),
        forge.clone(),
    );
    let saved = load(&sessions, &saved_root, Load::Saved).await;
    let deleted = load(&sessions, &deleted_root, Load::Saved).await;
    let unsaved = load(&sessions, &saved_root, Load::Unsaved).await;
    for session in [&saved, &deleted, &unsaved] {
        let inbox = sessions.pull_requests.inbox(&session.id, saved_root.path());
        watch_for_tests(&forge, &inbox).await;
    }
    // As the idle unload took them.
    for session in [&saved, &deleted] {
        sessions.loaded().remove(&session.id);
    }

    sessions.forget_deleted_pull_requests().await;

    let service = sessions.pull_requests.service();
    let watching = [&saved, &deleted, &unsaved].map(|session| service.watches(&session.id));
    assert_eq!(watching, [true, false, true]);
}
