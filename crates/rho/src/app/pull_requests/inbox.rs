//! One session's side of the PR service: [`PullRequestInbox`]. Its news
//! half has the shape of `tools/ui/host.rs`'s `SurfaceHost`
//! (`has_news`/`news_owned`/`take_news`/`restore_news`, plus `acknowledge`
//! for the durable removal); its tool half links and watches pull requests
//! for the session's agent.

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use rho_sdk::SessionId;
use tokio::process::Command;

use super::{
    model::{Mergeable, PrDetail, PrId, PrState, RepoId, Rollup, ThreadState},
    news,
    poller::WATCHES_PER_SESSION,
    record::LinkOrigin,
    service::{Entry, Shared},
    state::{CheckTally, Gate, News, WatchState},
    watch,
};

/// The host every session may read pull requests on.
const GITHUB: &str = "github.com";

/// How long `git remote get-url origin` may take to resolve `#N`. It reads
/// local config: 4 ms on 2026-11-22. A tripwire for a hung filesystem.
const GIT_DEADLINE: Duration = Duration::from_secs(5);

/// One session's handle on the PR service. Cheap to clone.
#[derive(Clone)]
pub(crate) struct PullRequestInbox {
    shared: Arc<Shared>,
    session: SessionId,
    /// The session's workspace, whose `origin` resolves `#N`.
    cwd: PathBuf,
}

/// News a turn took: [`Self::prompt`] for the model, and each item's
/// `News::card` for clients.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NewsBatch {
    /// Oldest first.
    pub(crate) news: Vec<News>,
    /// Older news the queue dropped, which the wake should mention.
    pub(crate) dropped: u32,
}

impl NewsBatch {
    pub(crate) fn is_empty(&self) -> bool {
        self.news.is_empty()
    }

    /// The wake's text for the model; see [`news::prompt`].
    pub(crate) fn prompt(&self) -> String {
        news::prompt(&self.news, self.dropped)
    }

    /// The seqs of its news.
    fn seqs(&self) -> Vec<u64> {
        self.news.iter().map(|news| news.seq).collect()
    }
}

/// A watch as `watch_pull_request` reports it: the baseline the agent
/// should handle before it ends its turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Watched {
    pub(crate) pr: PrId,
    pub(crate) url: String,
    pub(crate) state: PrState,
    /// The head commit's oid.
    pub(crate) head: String,
    pub(crate) checks: CheckCounts,
    /// Which checks must pass, and how many there are.
    pub(crate) gate: Gate,
    pub(crate) gate_checks: u32,
    pub(crate) mergeable: Mergeable,
    /// Comments and reviews, those on lines of code included, as far as
    /// the read counted them.
    pub(crate) comments: u32,
    /// Review threads not resolved, among those the read listed.
    pub(crate) unresolved_threads: u32,
    /// Whether this call started the watch; `false` when it was on already,
    /// and kept as it was.
    pub(crate) started: bool,
}

/// A PR's checks by how they stand. `total` may exceed the sum: a read
/// lists the first checks only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CheckCounts {
    pub(crate) total: u32,
    /// Passed, neutral or skipped.
    pub(crate) passed: u32,
    /// Running or waiting to run.
    pub(crate) running: u32,
    /// Failed or cancelled.
    pub(crate) failed: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Unwatched {
    pub(crate) pr: PrId,
    /// Whether it was watched; it stays linked either way.
    pub(crate) was_watched: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Linked {
    pub(crate) pr: PrId,
    pub(crate) url: String,
    /// Whether this call linked it; `false` when it was linked already.
    pub(crate) new: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Unlinked {
    pub(crate) pr: PrId,
    /// Whether it was linked.
    pub(crate) was_linked: bool,
}

/// One linked pull request, as `list_session_pull_requests` shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListedPullRequest {
    pub(crate) pr: PrId,
    pub(crate) url: String,
    pub(crate) origin: LinkOrigin,
    pub(crate) watched: bool,
    /// rho's last read of it; `None` before the first.
    pub(crate) last_read: Option<LastRead>,
    /// Why the last read of it failed, if it did.
    pub(crate) trouble: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LastRead {
    pub(crate) state: PrState,
    pub(crate) rollup: Rollup,
    /// How long ago it was read.
    pub(crate) age: Duration,
}

/// Why a PR tool call did nothing. Each names its cause, and a limit names
/// itself and the asked value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PullRequestError {
    #[error("the pull request is already {}, so there is nothing to watch; it stays linked to this session", .state.word())]
    AlreadyEnded { state: PrState },
    #[error("gh, GitHub's CLI, is not installed on the daemon's machine; install it and sign in with `gh auth login`, then try again")]
    NotInstalled,
    #[error("gh on the daemon's machine is not signed in to {host}; run `gh auth login --hostname {host}` there, then try again")]
    SignedOut { host: String },
    #[error("GitHub allows this account {remaining} more points until {}, at or below rho's floor of {floor}; try again then", reset_time(.resets_at))]
    Budget {
        remaining: u32,
        floor: u32,
        /// Unix seconds.
        resets_at: i64,
    },
    #[error("this session would watch {asked} pull requests, over its limit of {limit}; call unwatch_pull_request on one first")]
    TooManyWatches { limit: usize, asked: usize },
    /// In the forge's words.
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    InvalidTarget(String),
    #[error("could not read the pull request: {0}")]
    Read(String),
    #[error("{0}")]
    Store(String),
}

impl PullRequestInbox {
    pub(super) fn new(shared: Arc<Shared>, session: SessionId, cwd: PathBuf) -> Self {
        Self {
            shared,
            session,
            cwd,
        }
    }

    /// Whether news waits that no turn has taken.
    pub(crate) fn has_news(&self) -> bool {
        self.shared
            .state()
            .sessions
            .get(&self.session)
            .is_some_and(|entry| entry.waiting().next().is_some())
    }

    /// Completes when news arrives or is put back. Subscribe before
    /// checking [`Self::has_news`], so news in between is not missed.
    /// Makes the session an entry if it has none; a tick drops it again
    /// once nothing waits on it and it never had links or news.
    pub(crate) fn news_owned(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut state = self.shared.state();
        Arc::clone(&state.entry(&self.session).notify).notified_owned()
    }

    /// Takes the news that waits, oldest first, and marks it in flight: it
    /// stays queued, and on disk, until [`Self::acknowledge`], and is not
    /// offered again until [`Self::restore_news`].
    pub(crate) fn take_news(&self) -> NewsBatch {
        let mut state = self.shared.state();
        let Some(entry) = state.sessions.get_mut(&self.session) else {
            return NewsBatch::default();
        };
        let news: Vec<News> = entry.waiting().cloned().collect();
        if news.is_empty() {
            return NewsBatch::default();
        }
        entry.in_flight.extend(news.iter().map(|news| news.seq));
        NewsBatch {
            news,
            dropped: entry.record.news_dropped(),
        }
    }

    /// Puts back news a turn took and did not deliver. An empty batch
    /// returns at once, without telling waiters: a turn drops one on every
    /// pass, and telling would spin it.
    pub(crate) fn restore_news(&self, batch: &NewsBatch) {
        if batch.is_empty() {
            return;
        }
        let notify = {
            let mut state = self.shared.state();
            let Some(entry) = state.sessions.get_mut(&self.session) else {
                return;
            };
            entry.release(&batch.seqs());
            Arc::clone(&entry.notify)
        };
        notify.notify_waiters();
    }

    /// Removes delivered news for good, once the turn that carried it was
    /// applied. A failed save is logged and the removal stands in memory:
    /// after a crash the news is told once more, which at-least-once
    /// delivery allows.
    pub(crate) fn acknowledge(&self, batch: &NewsBatch) {
        if batch.is_empty() {
            return;
        }
        let mut state = self.shared.state();
        let Some(entry) = state.sessions.get_mut(&self.session) else {
            return;
        };
        let seqs = batch.seqs();
        entry.release(&seqs);
        entry.record.acknowledge(&seqs);
        let _ = self.shared.save(&self.session, &entry.record);
    }

    /// Watches `target` for the session, linking it first, at `now` (unix
    /// milliseconds). Reads the PR once as the watch's baseline, which is
    /// not news. A PR watched already keeps its watch as it is.
    pub(crate) async fn watch(&self, target: &str, now: u64) -> Result<Watched, PullRequestError> {
        self.shared.store()?;
        let pr = self.resolve(target).await?;
        let known = {
            let state = self.shared.state();
            let watching = self.watching(&state.sessions, &pr)?;
            let snapshot = state.snapshots.get(&pr);
            watching.then(|| snapshot.and_then(|s| s.detail.as_ref().map(|d| d.detail.clone())))
        };
        if let Some(Some(detail)) = known {
            return Ok(Watched::of(&detail, /*started*/ false));
        }
        let detail = self.shared.read_baseline(&pr, now).await?;
        let mut state = self.shared.state();
        let watching = self.watching(&state.sessions, &pr)?;
        let entry = state.entry(&self.session);
        let url = detail.summary.url.clone();
        let linked = entry
            .record
            .link(pr.clone(), url, LinkOrigin::Agent, now / 1000);
        let ended = matches!(detail.summary.state, PrState::Merged | PrState::Closed);
        let started = !watching && !ended;
        if started {
            if let Some(link) = entry.record.link_mut(&pr) {
                let baseline = watch::evaluate(&WatchState::new(now), &detail, now);
                link.watch = Some(baseline.next);
                link.told_head = Some(detail.summary.change.head_oid.clone());
            }
        }
        if linked || started {
            // The watch runs in memory; a failed save is logged.
            let _ = self.shared.save(&self.session, &entry.record);
        }
        if ended {
            return Err(PullRequestError::AlreadyEnded {
                state: detail.summary.state,
            });
        }
        Ok(Watched::of(&detail, started))
    }

    /// Stops watching `target`; it stays linked.
    pub(crate) async fn unwatch(&self, target: &str) -> Result<Unwatched, PullRequestError> {
        self.shared.store()?;
        let pr = self.resolve(target).await?;
        let mut state = self.shared.state();
        let Some(entry) = state.sessions.get_mut(&self.session) else {
            return Ok(Unwatched {
                pr,
                was_watched: false,
            });
        };
        let was_watched = entry
            .record
            .link_mut(&pr)
            .and_then(|link| link.watch.take())
            .is_some();
        if was_watched {
            // Polling stopped in memory; a failed save is logged.
            let _ = self.shared.save(&self.session, &entry.record);
        }
        Ok(Unwatched { pr, was_watched })
    }

    /// Links `target` to the session, unwatched, at `now` (unix
    /// milliseconds). Nothing is read until it is watched.
    pub(crate) async fn link(&self, target: &str, now: u64) -> Result<Linked, PullRequestError> {
        self.shared.store()?;
        let pr = self.resolve(target).await?;
        let url = pr.url();
        let mut state = self.shared.state();
        let entry = state.entry(&self.session);
        let new = entry
            .record
            .link(pr.clone(), url.clone(), LinkOrigin::Agent, now / 1000);
        if new {
            self.shared
                .save(&self.session, &entry.record)
                .map_err(PullRequestError::Store)?;
        }
        Ok(Linked { pr, url, new })
    }

    /// Unlinks `target` from the session, ending its watch if it has one.
    pub(crate) async fn unlink(&self, target: &str) -> Result<Unlinked, PullRequestError> {
        self.shared.store()?;
        let pr = self.resolve(target).await?;
        let mut state = self.shared.state();
        let Some(entry) = state.sessions.get_mut(&self.session) else {
            return Ok(Unlinked {
                pr,
                was_linked: false,
            });
        };
        let was_linked = entry.record.unlink(&pr);
        if was_linked {
            self.shared
                .save(&self.session, &entry.record)
                .map_err(PullRequestError::Store)?;
        }
        Ok(Unlinked { pr, was_linked })
    }

    /// The session's linked pull requests, in the order they were linked,
    /// with rho's last read of each as of `now` (unix milliseconds).
    pub(crate) fn list(&self, now: u64) -> Vec<ListedPullRequest> {
        let state = self.shared.state();
        let Some(entry) = state.sessions.get(&self.session) else {
            return Vec::new();
        };
        entry
            .record
            .links()
            .iter()
            .map(|link| {
                let snapshot = state.snapshots.get(&link.pr);
                let last_read = snapshot.and_then(|snapshot| {
                    let summary = snapshot.summary.as_ref()?;
                    Some(LastRead {
                        state: summary.state,
                        rollup: summary.change.rollup,
                        age: Duration::from_millis(now.saturating_sub(snapshot.read_at?)),
                    })
                });
                ListedPullRequest {
                    pr: link.pr.clone(),
                    url: link.url.clone(),
                    origin: link.origin,
                    watched: link.watch.is_some(),
                    last_read,
                    trouble: snapshot.and_then(|snapshot| snapshot.trouble.clone()),
                }
            })
            .collect()
    }

    /// Whether the session watches `pr`; refuses a watch past
    /// [`WATCHES_PER_SESSION`] when it does not.
    fn watching(
        &self,
        sessions: &HashMap<SessionId, Entry>,
        pr: &PrId,
    ) -> Result<bool, PullRequestError> {
        let Some(entry) = sessions.get(&self.session) else {
            return Ok(false);
        };
        if entry.record.watching().any(|(link, _)| link.pr == *pr) {
            return Ok(true);
        }
        let asked = entry.record.watching().count() + 1;
        if asked > WATCHES_PER_SESSION {
            return Err(PullRequestError::TooManyWatches {
                limit: WATCHES_PER_SESSION,
                asked,
            });
        }
        Ok(false)
    }

    /// The PR `target` names: a URL or `owner/name#N` as it is, and `#N`
    /// in the session's `origin` repository. Only a PR on github.com or on
    /// the host of the session's `origin` is read: `gh` would send that
    /// host the user's sign-in for it, so the agent can't pick the host.
    /// Git is asked for `origin` only when github.com does not settle it.
    async fn resolve(&self, target: &str) -> Result<PrId, PullRequestError> {
        let pr = match PrId::parse(target, None) {
            Ok(pr) if pr.repo.host == GITHUB => return Ok(pr),
            Ok(pr) => pr,
            Err(_) => {
                let origin = origin(&self.cwd).await;
                return PrId::parse(target, origin.as_ref())
                    .map_err(|why| PullRequestError::InvalidTarget(why.to_owned()));
            }
        };
        if origin(&self.cwd)
            .await
            .is_some_and(|origin| origin.host == pr.repo.host)
        {
            return Ok(pr);
        }
        Err(PullRequestError::InvalidTarget(format!(
            "rho only reads pull requests on {GITHUB} or this workspace's origin host, not {}",
            pr.repo.host
        )))
    }
}

impl fmt::Debug for PullRequestInbox {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PullRequestInbox")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl Watched {
    fn of(detail: &PrDetail, started: bool) -> Self {
        let tally = CheckTally::of(detail);
        let checks = CheckCounts {
            total: detail.checks_total,
            passed: tally.passed,
            running: tally.running,
            failed: tally.failed,
        };
        let (gate, gate_checks) = match watch::gate_of(&detail.checks) {
            // Every check, the unread ones too.
            (Gate::All, _) => (Gate::All, detail.checks_total),
            (Gate::Required, required) => (
                Gate::Required,
                u32::try_from(required.len()).unwrap_or(u32::MAX),
            ),
        };
        let thread_comments = detail.threads.iter().map(|thread| {
            u32::try_from(thread.comments.len())
                .unwrap_or(u32::MAX)
                .saturating_add(thread.more_comments)
        });
        let comments = u32::try_from(detail.posts.len())
            .unwrap_or(u32::MAX)
            .saturating_add(detail.older_posts)
            .saturating_add(thread_comments.fold(0, u32::saturating_add));
        Self {
            pr: detail.summary.id.clone(),
            url: detail.summary.url.clone(),
            state: detail.summary.state,
            head: detail.summary.change.head_oid.clone(),
            checks,
            gate,
            gate_checks,
            mergeable: detail.mergeable,
            comments,
            unresolved_threads: u32::try_from(
                detail
                    .threads
                    .iter()
                    .filter(|thread| thread.state != ThreadState::Resolved)
                    .count(),
            )
            .unwrap_or(u32::MAX),
            started,
        }
    }
}

/// The repository `cwd`'s `origin` remote names, if git answers in time.
async fn origin(cwd: &Path) -> Option<RepoId> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(cwd)
        .args(["remote", "get-url", "origin"])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(GIT_DEADLINE, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    RepoId::from_url(std::str::from_utf8(&output.stdout).ok()?)
}

/// `resets_at` (unix seconds) as a UTC time.
fn reset_time(resets_at: &i64) -> String {
    chrono::DateTime::from_timestamp(*resets_at, 0).map_or_else(
        || format!("unix time {resets_at}"),
        |at| at.format("%Y-%m-%d %H:%M UTC").to_string(),
    )
}
