//! The daemon's PR service, [`PullRequests`]: every session's record, the
//! snapshot of each pull request read (shared by all sessions that link
//! it), what clients look at and the branch lookups that serves, the forge
//! behind its read lock, and the channel that asks the daemon to wake
//! sessions with news.
//!
//! Locks, each taken before the next and never after it:
//! - `reader`, the async lock every forge read takes (see
//!   [`super::poller::Reader`]), held until what the read learned is kept
//!   in `state`, so a read that started later is never overwritten by an
//!   older one; a tick tells sessions their news only once it is released;
//! - a session's feed lock in the daemon, under which its reporter reads
//!   the session's view; nothing takes a feed lock while holding `state`;
//! - `state`, a plain mutex, never held across an await, and held while a
//!   record is saved so saves of one session never reorder;
//! - `budgets`, held only to read or keep one host's budget.
//!
//! Every change to what a session's view shows goes through
//! [`Shared::update`], which tells the daemon's reporter once `state` is
//! released.

use std::{
    collections::HashMap,
    fmt,
    future::Future,
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

use rho_sdk::SessionId;
use tokio::sync::{mpsc, Notify};

use super::{
    checkout::Checkout,
    due::{self, ShowOf},
    gh::{Forge, ReadError},
    inbox::{PullRequestError, PullRequestInbox},
    interest::{Interest, Lease, Shown},
    model::{BranchId, Found, PrDetail, PrId, PrSummary, RepoId},
    poller::{Budgets, Reader},
    record::SessionRecord,
    state::News,
    store::PullRequestStore,
    wire,
};

/// The daemon-wide PR service. Cheap to clone; every clone is the same
/// service.
#[derive(Clone)]
pub(crate) struct PullRequests(Arc<Shared>);

/// What [`PullRequests`] and each [`PullRequestInbox`] share.
pub(super) struct Shared {
    /// The record files, or why they could not be opened: then every tool
    /// operation fails with that cause.
    store: Result<PullRequestStore, String>,
    state: Mutex<State>,
    pub(super) reader: tokio::sync::Mutex<Reader>,
    /// Each host's budget as `reader` last kept it, readable without
    /// waiting on a read in flight. `reader` is its one writer.
    pub(super) budgets: Arc<Budgets>,
    /// Told after every [`Self::update`]: a session's view may differ.
    changed: Arc<Notify>,
    /// Wakes the poll loop before its period ends; see [`Self::leased`]
    /// and [`Self::poke_if_leased`].
    poke: Notify,
    wakes: mpsc::UnboundedSender<SessionId>,
    wake_requests: Mutex<Option<mpsc::UnboundedReceiver<SessionId>>>,
    merges: mpsc::UnboundedSender<MergedWatch>,
    merged_watches: Mutex<Option<mpsc::UnboundedReceiver<MergedWatch>>>,
}

/// A session's last watch ended because its pull request merged: it
/// watches no other, and none ended another way in the same read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergedWatch {
    pub(crate) session: SessionId,
    /// When rho found the merge, in unix milliseconds.
    pub(crate) at: u64,
}

/// A session a tick queued news for.
pub(super) struct Told {
    pub(super) session: SessionId,
    /// The session's [`Entry::notify`].
    pub(super) notify: Arc<Notify>,
    /// When the tick ran, in unix milliseconds, if the news tells that a
    /// watch ended merged, and of no watch that ended another way, and the
    /// session watches nothing else.
    pub(super) merged_at: Option<u64>,
}

/// What the service holds in memory.
#[derive(Default)]
pub(super) struct State {
    pub(super) sessions: HashMap<SessionId, Entry>,
    /// What the last reads found of each pull request a session links or
    /// a branch lookup found.
    pub(super) snapshots: HashMap<PrId, Snapshot>,
    /// What clients look at.
    pub(super) interest: Interest,
    /// What the last lookup of each branch a session has checked out found.
    pub(super) branches: HashMap<BranchId, BranchRead>,
    /// Each repository's default branch, which has no PR of its own, as
    /// the last read in it said.
    pub(super) default_branches: HashMap<RepoId, DefaultBranch>,
}

/// A repository's default branch as one read saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DefaultBranch {
    /// `None` when the repository has none.
    pub(super) name: Option<String>,
    /// When the read saw it, in unix milliseconds.
    pub(super) at: u64,
}

impl State {
    /// `session`'s entry, made empty when it has none.
    pub(super) fn entry(&mut self, session: &SessionId) -> &mut Entry {
        self.sessions
            .entry(session.clone())
            .or_insert_with(|| Entry::new(SessionRecord::default()))
    }

    /// The PR the last lookup of `branch` found.
    pub(super) fn found(&self, branch: &BranchId) -> Option<&PrId> {
        match self.branches.get(branch)?.found.as_ref()? {
            Found::Pr(summary) => Some(&summary.id),
            Found::None | Found::OnlyForks(_) => None,
        }
    }

    /// Makes `session`'s branch lookup due again when `pr`, just linked or
    /// watched, is in its repository and the last lookup did not find it:
    /// it may be the PR the session's agent has just opened for its branch.
    /// Otherwise clients would show it beside a branch that says it has
    /// none until the lookup's next period.
    pub(super) fn recheck_branch(&mut self, session: &SessionId, pr: &PrId) {
        let Some(branch) = self
            .interest
            .session(session)
            .and_then(|interest| interest.checkout().branch())
        else {
            return;
        };
        if branch.repo != pr.repo || self.found(branch) == Some(pr) {
            return;
        }
        if let Some(read) = self.branches.get_mut(branch) {
            read.asked_at = None;
        }
    }

    /// Whether `branch` is its repository's default branch, which has no
    /// PR of its own, as the last read in its repository said.
    pub(super) fn is_default(&self, branch: &BranchId) -> bool {
        self.default_seen_at(branch).is_some()
    }

    /// When the last read in `branch`'s repository saw it as the default
    /// branch, in unix milliseconds; `None` when it did not.
    pub(super) fn default_seen_at(&self, branch: &BranchId) -> Option<u64> {
        let default = self.default_branches.get(&branch.repo)?;
        (default.name.as_deref() == Some(branch.branch.as_str())).then_some(default.at)
    }

    /// Whether any session watches `pr`.
    pub(super) fn watched(&self, pr: &PrId) -> bool {
        self.sessions
            .values()
            .any(|entry| entry.record.watching().any(|(link, _)| link.pr == *pr))
    }
}

/// One session's side of [`State`].
pub(super) struct Entry {
    /// As saved, after every change.
    pub(super) record: SessionRecord,
    /// The seqs of news a turn took and has neither acknowledged nor put
    /// back: queued still, on disk too, and not offered again.
    pub(super) in_flight: Vec<u64>,
    /// Told when news arrives or is put back.
    pub(super) notify: Arc<Notify>,
    /// The newest merge a turn has told the session's agent of, no watch
    /// having ended another way since, until
    /// [`PullRequestInbox::take_merge_told`] takes it. In memory only: a
    /// daemon that stops first forgets it.
    pub(super) merge_told: Option<ToldMerge>,
    /// The record's [`SessionRecord::news_generation`] when the session
    /// last stopped watching a pull request itself, by an unwatch or an
    /// unlink, which queue no news: a merge queued before it did not end
    /// the session's last watch. In memory only, as `merge_told` is.
    pub(super) watch_dropped_at: Option<u64>,
}

/// A merge a turn told a session's agent of.
#[derive(Clone, Copy)]
pub(super) struct ToldMerge {
    /// When rho found it, in unix milliseconds.
    pub(super) at: u64,
    /// Its news's [`News::seq`].
    pub(super) seq: u64,
}

impl Entry {
    pub(super) fn new(record: SessionRecord) -> Self {
        Self {
            record,
            in_flight: Vec::new(),
            notify: Arc::new(Notify::new()),
            merge_told: None,
            watch_dropped_at: None,
        }
    }

    /// Queued news no turn has taken, oldest first.
    pub(super) fn waiting(&self) -> impl Iterator<Item = &News> + '_ {
        self.record
            .news()
            .iter()
            .filter(|news| !self.in_flight.contains(&news.seq))
    }

    /// Un-marks the news `seqs` names as in flight.
    pub(super) fn release(&mut self, seqs: &[u64]) {
        self.in_flight.retain(|seq| !seqs.contains(seq));
    }

    /// Whether dropping it loses nothing: no links, never any news (so its
    /// [news generation](SessionRecord::news_generation) cannot go back),
    /// and no one waiting on its `notify`, whom a new entry would not tell.
    /// The sessions that only ever asked for news without watching.
    pub(super) fn disposable(&self) -> bool {
        self.record.is_empty()
            && self.record.news_generation() == 0
            && Arc::strong_count(&self.notify) == 1
    }
}

/// What rho last read of one pull request.
#[derive(Default)]
pub(super) struct Snapshot {
    /// The newest summary, from a summary or a detail.
    pub(super) summary: Option<PrSummary>,
    /// The last detail a tick read, against which its watches were
    /// evaluated, or one read before any session watched it. Kept while a
    /// session watches it or a client shows it.
    pub(super) detail: Option<Detailed>,
    /// A detail of a watched PR a client's show read outside a tick, which
    /// no watch has been evaluated against: it answers shows only, while
    /// it is as new as [`Self::detail`] or newer (see
    /// [`Self::newest_detail`]). See `Shared::keep_unjudged`.
    pub(super) unjudged: Option<Detailed>,
    /// When anything was last read of it, in unix milliseconds.
    pub(super) read_at: Option<u64>,
    /// When a tick last asked for it, or a read outside a tick of a PR no
    /// session watches, in unix milliseconds, whether or not the read
    /// succeeded: what its cadence counts from, so a failing read is not
    /// asked again on every tick.
    pub(super) asked_at: Option<u64>,
    /// When a read of it outside a tick last failed, in unix milliseconds:
    /// a cached show retries it only once the shown cadence allows since
    /// then (see [`due::show_reads`]). Kept apart from [`Self::asked_at`],
    /// which shows never move for a watched PR, so they cannot postpone
    /// its watch.
    pub(super) show_failed_at: Option<u64>,
    /// Why the last read failed; `None` once one succeeds.
    pub(super) trouble: Option<Trouble>,
}

/// What the last lookup of one branch found.
#[derive(Debug, Default)]
pub(super) struct BranchRead {
    /// `None` until a lookup succeeds.
    pub(super) found: Option<Found>,
    /// When it was last looked up, in unix milliseconds, whether or not the
    /// lookup succeeded.
    pub(super) asked_at: Option<u64>,
    /// Why the last lookup failed; `None` once one succeeds.
    pub(super) trouble: Option<Trouble>,
}

/// Why the last read of a pull request or branch failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Trouble {
    /// The forge answered without it: no such PR, or a repository the
    /// account can't see, in the forge's words.
    Refused(String),
    /// The read got no answer.
    Failed(ReadError),
}

impl fmt::Display for Trouble {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(why) => formatter.write_str(why),
            Self::Failed(error) => write!(formatter, "{error}"),
        }
    }
}

/// A full read and when it was taken, in unix milliseconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Detailed {
    pub(crate) detail: PrDetail,
    pub(crate) at: u64,
}

/// Whether a client's show reads the forge before it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShowRead {
    /// Answer from the last read; the forge is read first only when there
    /// is no detail to answer with (see [`PullRequests::show`]).
    Cached,
    /// Read the forge now, as after the user's own action on the PR.
    Fresh,
}

/// What a client shows of a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShowTarget {
    /// The pull request of the branch its workspace has checked out.
    Branch,
    /// A pull request linked to it, as `inbox::resolve_pull_request`
    /// resolved the client's text.
    Linked(PrId),
}

impl Snapshot {
    pub(super) fn saw_summary(&mut self, summary: PrSummary, now: u64) {
        self.summary = Some(summary);
        self.read_at = Some(now);
        self.trouble = None;
    }

    pub(super) fn saw_detail(&mut self, detail: PrDetail, now: u64) {
        self.saw_summary(detail.summary.clone(), now);
        self.detail = Some(Detailed { detail, at: now });
        if self
            .unjudged
            .as_ref()
            .is_some_and(|unjudged| unjudged.at <= now)
        {
            self.unjudged = None;
        }
    }

    /// A show read `detail` of a watched PR at `now`, outside a tick: its
    /// summary is the newest, and its detail answers shows only.
    pub(super) fn saw_unjudged(&mut self, detail: PrDetail, now: u64) {
        self.saw_summary(detail.summary.clone(), now);
        self.unjudged = Some(Detailed { detail, at: now });
    }

    /// The newest detail kept. On equal times the unjudged one wins: a
    /// show stores it after the tick that read at the same time.
    pub(super) fn newest_detail(&self) -> Option<&Detailed> {
        match (&self.detail, &self.unjudged) {
            (Some(judged), Some(unjudged)) if unjudged.at >= judged.at => Some(unjudged),
            (Some(judged), _) => Some(judged),
            (None, unjudged) => unjudged.as_ref(),
        }
    }

    /// What a show answers with: the newest detail kept, while it reads
    /// as [`Self::summary`] does (see [`due::reads_as`]); a detail older
    /// than the summary sent beside it is no answer.
    pub(super) fn answer(&self) -> Option<&Detailed> {
        self.newest_detail().filter(|detailed| {
            self.summary
                .as_ref()
                .is_none_or(|summary| due::reads_as(&detailed.detail.summary, summary))
        })
    }

    /// A read of it outside a tick failed at `now` for `trouble`.
    pub(super) fn show_failed(&mut self, trouble: Trouble, now: u64) {
        self.trouble = Some(trouble);
        self.show_failed_at = Some(now);
    }

    /// Drops both details: no session watches the PR and no client shows it.
    pub(super) fn forget_details(&mut self) {
        self.detail = None;
        self.unjudged = None;
    }
}

impl Shared {
    pub(super) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `change` on the state, then tells the daemon's reporter, once
    /// the state is released, that a session's view may differ. The one
    /// way to change what a view shows. Telling when nothing changed is
    /// free: a session's feed sends only a set that differs.
    pub(super) fn update<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let changed = change(&mut self.state());
        self.changed.notify_waiters();
        changed
    }

    /// The store, or [`PullRequestError::Store`] with why it is missing.
    pub(super) fn store(&self) -> Result<&PullRequestStore, PullRequestError> {
        self.store
            .as_ref()
            .map_err(|cause| PullRequestError::Store(cause.clone()))
    }

    /// Saves `session`'s `record`; a failure is logged and returned, and
    /// what is in memory stands. Call with `state` held.
    pub(super) fn save(&self, session: &SessionId, record: &SessionRecord) -> Result<(), String> {
        let store = self.store.as_ref().map_err(Clone::clone)?;
        store.save(session, record).map_err(|error| {
            tracing::warn!(%session, %error, "could not save the session's pull request record");
            format!("could not save this session's pull request record: {error}")
        })
    }

    /// Tells each session in `told` that news arrived: its inbox's waiters,
    /// and the daemon's waker for unloaded sessions. A session's last
    /// watch that ended merged is told to the daemon as well, which marks
    /// the session on its own task: before, during or after the turn the
    /// news starts.
    pub(super) fn tell(&self, told: Vec<Told>) {
        for Told {
            session,
            notify,
            merged_at,
        } in told
        {
            if let Some(at) = merged_at {
                // A closed channel means the daemon takes no merges.
                let _ = self.merges.send(MergedWatch {
                    session: session.clone(),
                    at,
                });
            }
            notify.notify_waiters();
            // A closed channel means no waker runs: the news waits, queued,
            // for the session's next turn.
            let _ = self.wakes.send(session);
        }
    }

    /// A lease was taken or renewed: a new one wakes the poll loop, which
    /// reads what it made due. A renewal never pokes, so clients cannot
    /// raise the read rate.
    fn leased(&self, lease: Lease) {
        match lease {
            Lease::New => self.poke.notify_one(),
            Lease::Renewed => {}
        }
    }

    /// `session` linked a PR at `now`: when a lease on it lives, the PR is
    /// due at once, so the poll loop wakes. A link to a session no client
    /// looks at is not read until one does, or a watch asks.
    pub(super) fn poke_if_leased(&self, state: &State, session: &SessionId, now: u64) {
        if state.interest.leased(session, now) {
            self.poke.notify_one();
        }
    }
}

impl PullRequests {
    /// The service over the records in the daemon's `state_dir`, reading
    /// through `forge`. A store that cannot open is logged, and yields a
    /// service whose tool operations fail with that cause.
    pub(crate) fn open(state_dir: &Path, forge: Arc<dyn Forge>) -> Self {
        let (store, records) = match PullRequestStore::open(state_dir) {
            Ok(store) => {
                let records = store.load_all();
                (Ok(store), records)
            }
            Err(error) => {
                let dir = state_dir.display();
                tracing::warn!(%dir, %error, "could not open the pull request records");
                let cause = format!("could not open rho's pull request records in {dir}: {error}");
                (Err(cause), Vec::new())
            }
        };
        let sessions = records
            .into_iter()
            .map(|(session, record)| (session, Entry::new(record)))
            .collect();
        let (wakes, wake_requests) = mpsc::unbounded_channel();
        let (merges, merged_watches) = mpsc::unbounded_channel();
        let budgets = Arc::new(Budgets::default());
        Self(Arc::new(Shared {
            store,
            state: Mutex::new(State {
                sessions,
                ..State::default()
            }),
            reader: tokio::sync::Mutex::new(Reader::new(forge, Arc::clone(&budgets))),
            budgets,
            changed: Arc::new(Notify::new()),
            poke: Notify::new(),
            wakes,
            wake_requests: Mutex::new(Some(wake_requests)),
            merges,
            merged_watches: Mutex::new(Some(merged_watches)),
        }))
    }

    /// `session`'s handle. `cwd` resolves `#N` against its `origin`. The
    /// daemon builds one only for a session it can wake with news.
    pub(crate) fn inbox(&self, session: &SessionId, cwd: &Path) -> PullRequestInbox {
        PullRequestInbox::new(Arc::clone(&self.0), session.clone(), cwd.to_owned())
    }

    /// Polls every [`due::tick_period`], or sooner when a new lease pokes
    /// it, until `stopping` resolves; a tick under way stops with it.
    pub(crate) async fn run(&self, stopping: impl Future<Output = ()>) {
        tokio::pin!(stopping);
        loop {
            let period = due::tick_period(&self.0.state(), unix_ms());
            tokio::select! {
                () = &mut stopping => return,
                () = tokio::time::sleep(period) => {}
                () = self.0.poke.notified() => {}
            }
            tokio::select! {
                () = &mut stopping => return,
                () = self.tick(unix_ms()) => {}
            }
        }
    }

    /// One poll pass at `now` (unix milliseconds).
    pub(crate) async fn tick(&self, now: u64) {
        self.0.tick(now).await;
    }

    /// The sessions that got news, each once per tick, and those
    /// [`Self::request_wake`] named, for the daemon's waker of unloaded
    /// sessions. `Some` only the first time.
    pub(crate) fn take_wake_requests(&self) -> Option<mpsc::UnboundedReceiver<SessionId>> {
        self.0
            .wake_requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Each session whose last watch ended because its pull request merged,
    /// once, as the tick that read the merge queues its news; no other end
    /// of a watch is told here, nor a merge that leaves the session
    /// watching another pull request. `Some` only the first time.
    pub(crate) fn take_merged_watches(&self) -> Option<mpsc::UnboundedReceiver<MergedWatch>> {
        self.0
            .merged_watches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Asks the daemon's waker to wake `session`, on the channel
    /// [`Self::take_wake_requests`] drains, as news does.
    pub(crate) fn request_wake(&self, session: &SessionId) {
        // A closed channel means no waker runs, as in [`Shared::tell`].
        let _ = self.0.wakes.send(session.clone());
    }

    /// How many news items were ever queued for `session`: it grows each
    /// time news is, so the daemon can tell a batch it has not acted on.
    /// 0 for a session that never had news.
    pub(crate) fn news_generation(&self, session: &SessionId) -> u64 {
        self.0
            .state()
            .sessions
            .get(session)
            .map_or(0, |entry| entry.record.news_generation())
    }

    /// Whether `session` watches any pull request.
    pub(crate) fn watches(&self, session: &SessionId) -> bool {
        self.0
            .state()
            .sessions
            .get(session)
            .is_some_and(|entry| entry.record.watching().next().is_some())
    }

    /// Whether `session` has news no turn has taken.
    pub(crate) fn has_news(&self, session: &SessionId) -> bool {
        self.0
            .state()
            .sessions
            .get(session)
            .is_some_and(|entry| entry.waiting().next().is_some())
    }

    /// A client lists `session` at `now` (unix milliseconds); `checkout`
    /// is what its workspace has checked out. The lease lasts two periods
    /// of [`super::interest::LISTED_POLL`], and until then the session's
    /// linked PRs' summaries and its branch's lookup are read at that
    /// cadence. A new lease wakes the poller.
    pub(crate) fn list(&self, session: &SessionId, checkout: Checkout, now: u64) {
        let lease = self
            .0
            .update(|state| state.interest.list(session, checkout, now));
        self.0.leased(lease);
    }

    /// A client shows `target` of `session` at `now` (unix milliseconds);
    /// `checkout` is what its workspace has checked out. Answers the
    /// target's detail: the branch's PR's or the linked PR's, `None` while
    /// there is none to answer with (see [`Snapshot::answer`]). Then leases
    /// the target at
    /// [`super::interest::SHOWN_POLL`]: after the read, so the tick a new
    /// lease wakes finds the target just asked rather than reading it again.
    ///
    /// It reads the forge first, through the poller's lock and budget
    /// guard, when [`due::show_reads`] says so: when `read` is
    /// [`ShowRead::Fresh`], or there is no detail to answer with and the
    /// last attempt did not fail within the shown cadence. A read that
    /// fails is kept as the target's trouble, and a paused host as its
    /// budget, for the view to show; the answer is then the detail kept,
    /// if it still reads as the PR's summary.
    ///
    /// # Errors
    ///
    /// [`PullRequestError::InvalidTarget`] when `target` is a PR not linked
    /// to `session`.
    pub(crate) async fn show(
        &self,
        session: &SessionId,
        checkout: Checkout,
        target: ShowTarget,
        read: ShowRead,
        now: u64,
    ) -> Result<Option<Detailed>, PullRequestError> {
        let shown =
            match target {
                ShowTarget::Branch => Shown::Branch,
                ShowTarget::Linked(pr) => {
                    let linked =
                        self.0.state().sessions.get(session).is_some_and(|entry| {
                            entry.record.links().iter().any(|link| link.pr == pr)
                        });
                    if !linked {
                        return Err(PullRequestError::InvalidTarget(format!(
                            "{pr} is not linked to this session"
                        )));
                    }
                    Shown::Linked(pr)
                }
            };
        let of = match (&shown, checkout.branch()) {
            (Shown::Linked(pr), _) => Some(ShowOf::Linked(pr)),
            (Shown::Branch, Some(branch)) => Some(ShowOf::Branch(branch)),
            // On no branch of a github.com repository: nothing to look up.
            (Shown::Branch, None) => None,
        };
        let reads = of.is_some_and(|of| due::show_reads(&self.0.state(), of, read, now));
        let fresh = match of {
            Some(ShowOf::Linked(pr)) if reads => self
                .0
                .read_detail(pr, now)
                .await
                .ok()
                .map(|detail| Detailed { detail, at: now }),
            Some(ShowOf::Branch(branch)) if reads => self.0.read_branch(branch, now).await,
            Some(ShowOf::Linked(_) | ShowOf::Branch(_)) | None => None,
        };
        let detail = match (fresh, of) {
            (Some(fresh), _) => Some(fresh),
            (None, Some(of)) => kept_detail(&self.0.state(), of),
            (None, None) => None,
        };
        let lease = self
            .0
            .update(|state| state.interest.show(session, checkout, shown, now));
        self.0.leased(lease);
        Ok(detail)
    }

    /// Completes the next time any session's view may have changed.
    /// Subscribe before reading the views, so a change in between is not
    /// missed.
    pub(crate) fn changes(&self) -> impl Future<Output = ()> + Send + 'static {
        Arc::clone(&self.0.changed).notified_owned()
    }

    /// `session`'s pull requests as clients see them at `now` (unix
    /// milliseconds), from what was last read; see [`wire::session`].
    /// Never waits on a forge read.
    pub(crate) fn view(&self, session: &SessionId, now: u64) -> rho_protocol::SessionPullRequests {
        let budgets = &self.0.budgets;
        wire::session(&self.0.state(), |host| budgets.paused(host, now), session)
    }

    /// `session` was deleted: drops its record, from memory and disk, and
    /// what clients looked at of it.
    pub(crate) fn forget(&self, session: &SessionId) {
        let mut state = self.0.state();
        state.sessions.remove(session);
        state.interest.forget(session);
        if let Ok(store) = &self.0.store {
            if let Err(error) = store.forget(session) {
                tracing::warn!(%session, %error, "could not delete the session's pull request record");
            }
        }
    }
}

/// The detail kept of what `of` names: the linked PR's, or that of the PR
/// the branch's last lookup found.
fn kept_detail(state: &State, of: ShowOf<'_>) -> Option<Detailed> {
    let pr = match of {
        ShowOf::Linked(pr) => pr,
        ShowOf::Branch(branch) => state.found(branch)?,
    };
    state.snapshots.get(pr)?.answer().cloned()
}

impl fmt::Debug for PullRequests {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PullRequests")
            .finish_non_exhaustive()
    }
}

/// Now, in unix milliseconds, as every PR time is kept.
///
/// Read only at the edges: the poll loop, the daemon's request handlers
/// and reporter, and the agent's PR tools. Everything below them takes
/// `now`, so tests pass their own.
pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
