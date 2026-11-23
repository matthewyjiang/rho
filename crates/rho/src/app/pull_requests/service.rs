//! The daemon's PR service, [`PullRequests`]: every session's record, the
//! snapshot of each pull request read (shared by all sessions that link
//! it), the forge behind its read lock, and the channel that asks the
//! daemon to wake sessions with news.
//!
//! Locks: `state` is a plain mutex, never held across an await, and is
//! held while a record is saved so saves of one session never reorder.
//! `reader` is the async lock every forge read takes (see
//! [`super::poller::Reader`]); it is taken before `state`, never after.

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
    gh::Forge,
    inbox::{PullRequestError, PullRequestInbox},
    model::{PrDetail, PrId, PrSummary},
    poller::{Reader, WATCH_POLL},
    record::SessionRecord,
    state::News,
    store::PullRequestStore,
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
    wakes: mpsc::UnboundedSender<SessionId>,
    wake_requests: Mutex<Option<mpsc::UnboundedReceiver<SessionId>>>,
}

/// What the service holds in memory.
#[derive(Default)]
pub(super) struct State {
    pub(super) sessions: HashMap<SessionId, Entry>,
    /// What the last reads found of each linked pull request.
    pub(super) snapshots: HashMap<PrId, Snapshot>,
}

impl State {
    /// `session`'s entry, made empty when it has none.
    pub(super) fn entry(&mut self, session: &SessionId) -> &mut Entry {
        self.sessions
            .entry(session.clone())
            .or_insert_with(|| Entry::new(SessionRecord::default()))
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
}

impl Entry {
    pub(super) fn new(record: SessionRecord) -> Self {
        Self {
            record,
            in_flight: Vec::new(),
            notify: Arc::new(Notify::new()),
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
    pub(super) detail: Option<Detailed>,
    /// When anything was last read of it, in unix milliseconds.
    pub(super) read_at: Option<u64>,
    /// Why the last read failed; `None` once one succeeds.
    pub(super) trouble: Option<String>,
}

/// A full read and when it was taken, in unix milliseconds.
pub(super) struct Detailed {
    pub(super) detail: PrDetail,
    pub(super) at: u64,
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
    }
}

impl Shared {
    pub(super) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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
    /// and the daemon's waker for unloaded sessions.
    pub(super) fn tell(&self, told: Vec<(SessionId, Arc<Notify>)>) {
        for (session, notify) in told {
            notify.notify_waiters();
            // A closed channel means no waker runs: the news waits, queued,
            // for the session's next turn.
            let _ = self.wakes.send(session);
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
        Self(Arc::new(Shared {
            store,
            state: Mutex::new(State {
                sessions,
                snapshots: HashMap::new(),
            }),
            reader: tokio::sync::Mutex::new(Reader::new(forge)),
            wakes,
            wake_requests: Mutex::new(Some(wake_requests)),
        }))
    }

    /// `session`'s handle. `cwd` resolves `#N` against its `origin`. The
    /// daemon builds one only for a session it can wake with news.
    pub(crate) fn inbox(&self, session: &SessionId, cwd: &Path) -> PullRequestInbox {
        PullRequestInbox::new(Arc::clone(&self.0), session.clone(), cwd.to_owned())
    }

    /// Polls every [`WATCH_POLL`] until `stopping` resolves; a tick under
    /// way stops with it.
    pub(crate) async fn run(&self, stopping: impl Future<Output = ()>) {
        tokio::pin!(stopping);
        loop {
            tokio::select! {
                () = &mut stopping => return,
                () = tokio::time::sleep(WATCH_POLL) => {}
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

    /// `session` was deleted: drops its record, from memory and disk.
    pub(crate) fn forget(&self, session: &SessionId) {
        let mut state = self.0.state();
        state.sessions.remove(session);
        if let Ok(store) = &self.0.store {
            if let Err(error) = store.forget(session) {
                tracing::warn!(%session, %error, "could not delete the session's pull request record");
            }
        }
    }
}

impl fmt::Debug for PullRequests {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PullRequests")
            .finish_non_exhaustive()
    }
}

/// The wall clock in unix milliseconds: the poller's only clock.
/// Now, in unix milliseconds, as every PR time is kept.
pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
