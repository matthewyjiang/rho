//! The sessions the daemon hosts, prompted turn by turn, whose turns run in
//! the daemon and outlive the client that sent them.
//!
//! A loaded session owns its [`SessionRuntime`] and the [`SessionHandle`]
//! that cancels and steers it without waiting for the turn, and its turns
//! are one job in the registry. Requests name the session by its session
//! id. A prompt or an attach for a saved session the daemon has not loaded,
//! such as one an earlier daemon ran or one the TUI saved, reopens it from
//! its file in its own workspace, recording its turns on the session's last
//! job (#1661). Cancel, steer, unsteer, answer, hold, and configure never
//! load a session: one that is not loaded has no turn to act on, and a
//! client attaches before it changes settings. A steer answers with its id,
//! which a client withdraws it by; see [`steer`]. A prompt answers with its
//! turn's id, which the turn's states and end carry, and may name a key
//! that makes sending it again safe; see [`start`].
//!
//! A session whose host runs its children as its own threads (T3 Code) is
//! woken through that host once background work that outlived its turn
//! finishes; see [`t3_wake`].
//!
//! Clients attach to a loaded session to follow its events (#1657); see
//! [`feed`]. A client that only reads a session observes it instead, which
//! never loads it; see [`observe`]. A client may change the session's settings, which apply
//! between turns, a change sent during a turn once it ends, and which the
//! session keeps with its file; see [`configure`] and [`options`]. A
//! session that sits idle, with no turn under way and no client attached,
//! is unloaded until a request reopens it, or discarded when it has saved
//! nothing yet; see [`unload`]. Clients list, rename, and delete saved
//! sessions, loaded or not; see [`catalog`].
//!
//! Once the daemon starts stopping, or restarting in place (#1713), it
//! refuses new sessions and prompts and detaches every client, telling it
//! which; then it stops the turns under way, waits for them to save, and
//! records them interrupted.

use std::{
    collections::HashMap,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::Duration,
};

use rho_protocol::{
    DetachReason, ErrorCode, RpcError, SessionAnswerParams, SessionAnswerResponse,
    SessionAttachParams, SessionAttachResponse, SessionCancelParams, SessionCancelResponse,
    SessionHoldParams, SessionHoldResponse, SessionNewParams, SessionNewResponse, Subscription,
    TurnOutcome,
};
use rho_sdk::{CancellationToken, SessionId};

use super::{
    jobs::{
        Admission, JobEvent, JobId, JobRecord, JobRegistry, RegisterError, SessionJob, TurnEnd,
        TurnRecord,
    },
    subagents::Subagents,
    wire::{session_to_wire, state_event},
};
use crate::{
    app::{
        session_events::LiveEvents,
        session_runtime::{CredentialFailure, SessionHandle, SessionRuntime},
    },
    config::Config,
};

mod budget;
mod catalog;
mod configure;
mod cost;
mod feed;
mod goal;
mod grant;
mod inputs;
mod mcp;
mod next_turn;
mod observe;
mod open;
mod options;
mod queued;
mod reopen;
mod setup;
mod start;
mod steer;
mod store;
mod t3_wake;
#[cfg(test)]
mod test_events;
#[cfg(test)]
mod test_sessions;
mod turn;
mod unload;

use super::settings::ConfigStore;
use feed::{Attached, Feed};
use inputs::Inputs;
pub(super) use open::session_environment;
use open::{open_session, OpenWith, OpenedSession, Opening, Workspace};
use queued::Queued;
use reopen::ReopenLocks;
use start::Entered;
use t3_wake::{T3Wake, WakeDir};
use turn::{PromptStart, TurnLimit, TurnTask};
use unload::Residency;
pub(in crate::app) use unload::UNLOAD_IDLE_AFTER;

/// How long the daemon's stop waits for its turns to save once it was
/// asked to stop, before it records them interrupted and exits anyway.
//
// Tripwire, not measured: a cancelled turn saves in milliseconds, so this
// fires only on a turn stuck in a tool that ignores the cancel. It runs from
// the stop request and stays under `DAEMON_STOP_TIMEOUT` (5 s) with room for
// the rest of the exit (44 ms max measured), so `rho daemon stop` still sees
// the daemon go. `rho daemon restart` drains the same way; an upgrade waits
// for the daemon to be idle first, so it has nothing to drain.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(4);

/// Every session the daemon has loaded, and the job registry that records
/// their turns.
pub(super) struct Sessions {
    jobs: Arc<JobRegistry>,
    /// Lock order: a session's turn lock, then this; never the reverse.
    loaded: Mutex<HashMap<SessionId, Arc<LoadedSession>>>,
    /// The saved sessions being reopened, one at a time each, and those the
    /// idle unload or a delete is tearing down, which a reopen waits for. A
    /// delete holds a session's until its files are gone, so a reopen then
    /// finds nothing saved.
    reopening: ReopenLocks,
    /// The daemon-wide budget the sessions' subagents share, and their jobs.
    subagents: Subagents,
    /// The daemon's settings, which sessions open and turn with.
    settings: Arc<ConfigStore>,
    /// Cancelled once the daemon starts stopping ([`Self::begin_stop`]).
    stopping: CancellationToken,
    /// Read-held from each idle unload decision through its teardown, and
    /// through each delete; the stop write-locks it to wait for them.
    unloading: tokio::sync::RwLock<()>,
    /// Where the sessions' T3 wake watchers keep their files (see
    /// [`t3_wake`]).
    wakes: WakeDir,
}

/// A session this daemon loaded.
struct LoadedSession {
    /// The session's id, which its usage is recorded under in the usage
    /// ledger.
    id: SessionId,
    /// The job that records the session's turns, in `jobs`.
    job: JobId,
    jobs: Arc<JobRegistry>,
    /// Held by the turn under way and by the change applying; taken when
    /// the daemon stops. Fair, so turns and changes take it in the order
    /// they joined its queue, which each does under the turn lock (see
    /// [`Queued`]).
    runtime: Arc<tokio::sync::Mutex<Option<SessionRuntime>>>,
    handle: SessionHandle,
    /// The config the session's turns read: the one it opened with, which
    /// each turn brings up to date (see [`next_turn`]).
    config: Mutex<next_turn::SessionConfig>,
    /// The daemon's settings, which each turn reads the global config from.
    settings: Arc<ConfigStore>,
    /// The environment the session's children see, which each prompt
    /// replaces with its client's (#1659).
    environment: crate::session_environment::SessionEnvironment,
    /// The directory the session works in.
    workspace: PathBuf,
    /// The turn's cancel and the session's residency. A prompt's start, an
    /// attach, a turn's end, a cancel, and the idle unload are ordered by
    /// this lock.
    turn: Mutex<TurnSlot>,
    /// The session's events, as attached clients follow them.
    feed: Feed,
    /// Shared inventory handles; reads never wait for the active turn.
    mcp: Mutex<crate::app::session_mcp::SessionMcp>,
    /// Goal lifecycle shared with control requests while the runtime is busy.
    goal: Mutex<goal::GoalControl>,
    /// Turns the runtime's events into the feed's; only the turn under way
    /// and its steers use it, each briefly and never across an await.
    live: Mutex<LiveEvents>,
    /// The input requests the turn under way has open (#1660).
    inputs: Mutex<Inputs>,
    /// The watchers that wake the idle session through its host, present
    /// exactly when its host wakes it (see [`t3_wake`]). Only turns, which
    /// take the runtime one at a time, lock it.
    wake: Option<tokio::sync::Mutex<T3Wake>>,
}

/// The state a session's turn lock guards.
struct TurnSlot {
    /// The cancel of the session's current or last turn. A prompt replaces
    /// it as its turn starts, and a cancel trips it and the session's
    /// handle, so a cancel never reaches a turn it did not see start. A
    /// turn that arms a T3 wake watcher replaces it once more as its run
    /// ends, so a later cancel still reaches its host calls.
    cancel: CancellationToken,
    /// What the session's current or last turn runs, which decides what a
    /// prompt sent while it runs does. A prompt sets it as its turn starts.
    kind: TurnKind,
    /// The session's current or last turn that a prompt naming a key
    /// started, which a prompt sent again with that key answers as (see
    /// [`start`]). A turn starting replaces it.
    keyed: Option<KeyedTurn>,
    /// Whether the session is in use, idle, or unloaded, as the idle unload
    /// tracks it.
    residency: Residency,
    /// What holds the session, if anything: neither a delete nor the idle
    /// unload takes a held session. Set and freed by [`Held`].
    hold: Option<Hold>,
}

/// A turn a prompt naming `key` started, as a prompt sent again with the
/// key finds it.
struct KeyedTurn {
    key: rho_protocol::IdempotencyKey,
    id: rho_protocol::TurnId,
    /// How the turn began, once it has.
    start: tokio::sync::watch::Receiver<PromptStart>,
}

impl KeyedTurn {
    /// The turn job `job` last ran, as an earlier daemon or an earlier load
    /// recorded it, if a prompt naming a key started it and it began: one
    /// that did not begin ran nothing, so its key starts a turn again.
    fn recorded(job: &JobRecord) -> Option<Self> {
        let TurnRecord { id, key, began } = job.turn.clone()?;
        let key = key.filter(|_| began)?;
        // The sender is dropped: the turn began, and nothing changes that.
        let (_, start) = tokio::sync::watch::channel(PromptStart::Began);
        Some(Self { key, id, start })
    }
}

/// What a session's turn runs, as far as a prompt sent meanwhile goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TurnKind {
    /// A prompt: one sent meanwhile is refused as busy.
    Prompt,
    /// A T3 wake watcher's wait (see [`t3_wake`]), which the watch sets: one
    /// sent meanwhile, as T3 Code sends once restarted after its `rho acp`
    /// was killed, cancels the wait and runs once it ended, resuming it.
    WakeWait,
}

/// What holds a session: its queued settings changes or its delete, which
/// exclude each other. The turn lock is a std mutex no request holds across
/// its awaits, so a request marks its hold in [`TurnSlot::hold`] instead of
/// holding the lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    /// This many changes to the session's settings are queued on its
    /// runtime or applying (see [`configure`]). A prompt still starts a
    /// turn, which queues on the runtime behind them.
    Configuring(NonZeroUsize),
    /// A delete is unloading the session to delete its files; no turn
    /// starts and no change queues meanwhile.
    Deleting,
}

/// A request's part of a session's [`Hold`], freed when dropped, whether
/// the request finished, failed, or was dropped.
struct Held(Arc<LoadedSession>);

impl Held {
    /// Counts one more change queued on `session`, or `None` while a delete
    /// holds it. `turn` is `session`'s turn lock, held.
    fn change(session: &Arc<LoadedSession>, turn: &mut TurnSlot) -> Option<Self> {
        let queued = match turn.hold {
            None => NonZeroUsize::MIN,
            Some(Hold::Configuring(queued)) => queued
                .checked_add(1)
                .expect("each queued change is a request in flight"),
            Some(Hold::Deleting) => return None,
        };
        turn.hold = Some(Hold::Configuring(queued));
        Some(Self(Arc::clone(session)))
    }

    /// Holds `session` for its delete. `turn` is `session`'s turn lock,
    /// held, with nothing holding the session.
    fn delete(session: &Arc<LoadedSession>, turn: &mut TurnSlot) -> Self {
        debug_assert_eq!(turn.hold, None, "a delete holds only a free session");
        turn.hold = Some(Hold::Deleting);
        Self(Arc::clone(session))
    }
}

impl Drop for Held {
    /// Takes the session's turn lock, a std mutex: never drop a `Held`
    /// while holding that lock's guard, which would deadlock.
    fn drop(&mut self) {
        let mut turn = self.0.turn();
        // Changes and a delete exclude each other, so the hold in place is
        // the kind this one is part of.
        turn.hold = match turn.hold {
            Some(Hold::Configuring(queued)) => {
                NonZeroUsize::new(queued.get() - 1).map(Hold::Configuring)
            }
            Some(Hold::Deleting) | None => None,
        };
    }
}

/// Times out input request `id` of `session` after `after`, unless the
/// request closed or was held by then, or the daemon dropped the session.
async fn time_out(session: Weak<LoadedSession>, id: rho_protocol::InputRequestId, after: Duration) {
    tokio::time::sleep(after).await;
    if let Some(session) = session.upgrade() {
        session.change_inputs(|inputs| inputs.time_out(id));
    }
}

/// The rows `runtime` has saved, replayed through `live` as the feed's
/// events, or `None` when it has saved none yet or they could not be read.
fn saved_replay(
    live: &mut LiveEvents,
    runtime: &SessionRuntime,
    job: &JobId,
) -> Option<Vec<rho_protocol::SessionEvent>> {
    let saved = runtime.saved_history()?;
    match saved.display_rows() {
        Ok(rows) => live.replay(&rows, saved.cwd(), saved.tools()),
        Err(error) => {
            tracing::warn!(
                %job,
                error = format!("{error:#}"),
                "could not read the session's saved rows; a reset replays only the turns since, live, until a later turn saves"
            );
            None
        }
    }
}

/// The feed of a session opened as `runtime` with `config`, whose turns are
/// recorded on `job`, and the mapper that replayed it: the feed begins with
/// the conversation the session has saved and how the job's last turn
/// ended, which a new session has neither of, so a client attaching to a
/// reopened session learns that end (#1661).
fn opening_feed(
    job: &JobRecord,
    runtime: &SessionRuntime,
    config: &Config,
    auths: &[String],
) -> (Feed, LiveEvents) {
    let mut live = LiveEvents::new(
        runtime.workspace_root(),
        runtime.tools(),
        runtime.configured_context_window(),
    );
    // Titles are not saved with the transcript, so the feed starts with the
    // one the session's file has.
    let last = job.moved();
    let feed = Feed::new(
        (&last.state).into(),
        last.turn.clone(),
        runtime.stored_title(),
        options::daemon_options(runtime, config, auths),
    );
    if let Some(saved) = saved_replay(&mut live, runtime, &job.id) {
        feed.end_turn(saved);
    }
    // An outcome is recorded with the turn it is of.
    if let (Some(outcome), Some(turn)) = (job.outcome.clone(), last.turn.clone()) {
        feed.publish([
            rho_protocol::SessionEvent::TurnEnded { turn, outcome },
            state_event(&last),
        ]);
    }
    (feed, live)
}

impl LoadedSession {
    /// A session opened as `opened`, whose turns are recorded on `job` in
    /// `jobs`: a new job for a new session, the last one for a reopened one.
    /// Its feed is its [`opening_feed`]. Its T3 wake watchers, when its host wakes it, keep their files in
    /// `wakes`.
    fn new(
        job: &JobRecord,
        jobs: Arc<JobRegistry>,
        opened: OpenedSession,
        auths: &[String],
        wakes: &WakeDir,
        settings: &Arc<ConfigStore>,
    ) -> Self {
        let OpenedSession {
            runtime,
            config,
            global,
            environment,
        } = opened;
        let (feed, live) = opening_feed(job, &runtime, &config, auths);
        let keyed = KeyedTurn::recorded(job);
        let job = job.id.clone();
        Self {
            id: runtime.session_id().clone(),
            wake: T3Wake::of(&runtime, wakes).map(tokio::sync::Mutex::new),
            handle: runtime.handle(),
            workspace: runtime.workspace_root().to_path_buf(),
            live: Mutex::new(live),
            mcp: Mutex::new(crate::app::session_mcp::SessionMcp::of(&runtime)),
            feed,
            goal: Mutex::default(),
            job,
            jobs,
            runtime: Arc::new(tokio::sync::Mutex::new(Some(runtime))),
            inputs: Mutex::new(Inputs::new(
                config
                    .questionnaire
                    .timeout_seconds
                    .map(|seconds| Duration::from_secs(seconds.get())),
            )),
            config: next_turn::SessionConfig::new(config, global),
            settings: Arc::clone(settings),
            environment,
            turn: Mutex::new(TurnSlot {
                cancel: CancellationToken::new(),
                kind: TurnKind::Prompt,
                keyed,
                residency: Residency::InUse,
                hold: None,
            }),
        }
    }

    /// Changes the session's open input requests with `change`, tells
    /// attached clients what it opened and resolved, moves the job between
    /// running and waiting for input to match, and only then hands the
    /// turns the answers it accepted. All under the requests' lock, so
    /// clients see requests open and close, and the job's state follow
    /// them, in the order they happened, and before anything the turn does
    /// with an answer. Starts the timeouts the change opened.
    fn change_inputs<T>(self: &Arc<Self>, change: impl FnOnce(&mut Inputs) -> T) -> T {
        // A panic mid-change leaves the requests as they were or with one
        // request moved, which the turn's end clears.
        let mut inputs = self.inputs.lock().unwrap_or_else(PoisonError::into_inner);
        let was_waiting = inputs.waiting();
        let result = change(&mut inputs);
        self.feed.publish(inputs.take_events());
        match (was_waiting, inputs.waiting()) {
            (false, true) => self.record(JobEvent::InputOpened),
            (true, false) => self.record(JobEvent::InputSettled),
            (false, false) | (true, true) => {}
        }
        inputs.send_answers();
        for (id, after) in inputs.take_timeouts() {
            tokio::spawn(time_out(Arc::downgrade(self), id, after));
        }
        result
    }

    /// Records that the turn, queued for capacity, has its slot: `running`
    /// from then on. Under the turn lock, as the turn's start and end are
    /// told (see `Sessions::start_turn`).
    fn admitted(&self) {
        let _turn = self.turn();
        self.record(JobEvent::Admitted);
    }

    /// Applies `event` to the session's job and tells attached clients the
    /// state it left the job in.
    fn record(&self, event: JobEvent) {
        match self.jobs.record(&self.job, event) {
            Ok(moved) => self.feed.publish([state_event(&moved)]),
            Err(error) => {
                tracing::warn!(job = %self.job, %error, "could not record a turn's progress");
            }
        }
    }

    /// Records that turn `turn` ended as `end`, keeping `outcome` on the job
    /// for a later daemon to replay, and tells attached clients `outcome`,
    /// if any, then the state it left the job in, in one publish, so no
    /// client's replay holds one without the other (#1669).
    fn end_turn(&self, turn: rho_protocol::TurnId, outcome: Option<TurnOutcome>, end: TurnEnd) {
        let ended = outcome
            .clone()
            .map(|outcome| rho_protocol::SessionEvent::TurnEnded { turn, outcome });
        match self.jobs.end_turn(&self.job, end, outcome) {
            Ok(moved) => self
                .feed
                .publish(ended.into_iter().chain([state_event(&moved)])),
            Err(error) => {
                tracing::warn!(job = %self.job, %error, "could not record a turn's end");
                self.feed.publish(ended);
            }
        }
    }

    fn live(&self) -> MutexGuard<'_, LiveEvents> {
        // A panic while mapping one event leaves the mapper as it was
        // between events, at worst missing that event's bookkeeping.
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn turn(&self) -> MutexGuard<'_, TurnSlot> {
        // Its fields are only ever replaced or cancelled whole, so a panic
        // cannot leave it half-changed.
        self.turn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The turn lock of a request that uses the session, marked in use, or
    /// `None` once the idle unload took the session: the request looks it
    /// up again instead, which reopens it if it was saved.
    fn enter(&self) -> Option<MutexGuard<'_, TurnSlot>> {
        let mut turn = self.turn();
        turn.residency.mark_in_use().then_some(turn)
    }

    /// Cancels the turn under way, if any, even one that has not begun its
    /// prompt yet.
    fn cancel_turn(&self) {
        self.cancel_turn_under(&self.turn());
    }

    /// [`Self::cancel_turn`], with `turn`, the session's turn lock, held.
    fn cancel_turn_under(&self, turn: &TurnSlot) {
        // Both under the turn lock, which a prompt takes to start its turn,
        // so the handle's cancel cannot reach a turn started after this
        // cancel. Lock order: the turn lock, then the prompt gate's
        // slot inside `handle.cancel()`; nothing takes them the other way.
        turn.cancel.cancel();
        self.cancel_goal();
        self.handle.cancel();
    }
}

impl Sessions {
    /// No sessions yet; their turns are recorded in `jobs`, and they keep
    /// their T3 wake watchers' files under `rho_home`, the daemon's Rho
    /// home.
    pub(super) fn new(jobs: Arc<JobRegistry>, rho_home: &Path) -> Self {
        let stopping = CancellationToken::new();
        let subagents = Subagents::new(Arc::clone(&jobs), stopping.clone());
        Self {
            settings: Arc::new(ConfigStore::new(rho_home, subagents.concurrency())),
            subagents,
            jobs,
            loaded: Mutex::default(),
            reopening: ReopenLocks::default(),
            stopping,
            unloading: tokio::sync::RwLock::new(()),
            wakes: WakeDir::in_home(rho_home),
        }
    }

    /// The daemon's settings.
    pub(super) fn settings(&self) -> &ConfigStore {
        &self.settings
    }

    /// The sessions' subagents: the daemon-wide budget and its queue.
    pub(super) fn subagents(&self) -> &Subagents {
        &self.subagents
    }

    /// Starts the daemon's stop, or its restart: from now on no session
    /// opens, no turn starts, no client attaches, and every client attached
    /// to a session, even by an attach that raced the stop, is detached as
    /// `told`, `daemon_stopping` or `restarting`. Call before the transport
    /// closes the connections, so each client learns why. Calling again
    /// keeps what the first call told.
    pub(super) fn begin_stop(&self, told: DetachReason) {
        // Under the lock `add` checks `stopping` under, so no session is
        // added after this with its feed open.
        let loaded = self.loaded();
        self.stopping.cancel();
        for session in loaded.values() {
            session.feed.close(told);
        }
    }

    /// Opens a new session in `cwd`, as `agent`, with the client's `setup`,
    /// and registers the job of its turns.
    pub(super) async fn open_new(
        &self,
        params: SessionNewParams,
    ) -> Result<SessionNewResponse, RpcError> {
        let SessionNewParams {
            cwd,
            environment,
            agent,
            setup,
            parent,
            grant,
        } = params;
        self.refuse_when_stopping()?;
        let workspace = Workspace::parse(&cwd)?;
        // Resolved before the session opens, so a malformed parent opens
        // nothing.
        let started_by = parent.map(|parent| self.job_writing(&parent)).transpose()?;
        let with = OpenWith::under(grant::asked(grant, &environment)?, &environment, setup)?;
        let job = JobId::generate();
        let opened = open_session(
            Opening::New { workspace, agent },
            &with,
            &self.subagents,
            &self.settings,
            &job,
        )
        .await?;
        let session = opened.runtime.session_id().clone();
        let record = JobRecord {
            // A session started from one no daemon job writes still shares
            // the budget, with no parent to show.
            admission: match started_by {
                Some(_) => Admission::SharedBudget,
                None => Admission::Immediate,
            },
            parent: started_by.flatten(),
            grant: with.grant,
            ..JobRecord::new_session(
                job.clone(),
                opened.runtime.workspace_root().to_path_buf(),
                session.clone(),
            )
        };
        let added = self.add(SessionJobRecord::Register(record), opened).await?;
        tracing::info!(%session, %job, "opened session");
        Ok(SessionNewResponse {
            session: session_to_wire(session),
            options: added.feed.options(),
        })
    }

    /// The job writing `session`, a new session's parent (#1714): the one
    /// under way, as the turn of a session that starts another is, else the
    /// latest job of its turns. `None` for a session no job of this daemon
    /// writes, such as one an in-process rho runs.
    fn job_writing(&self, session: &rho_protocol::SessionId) -> Result<Option<JobId>, RpcError> {
        Ok(
            match self.jobs.session_job(&reopen::parse_session(session)?) {
                SessionJob::Busy(job) => Some(job),
                SessionJob::Last(record) => Some(record.id),
                SessionJob::New => None,
            },
        )
    }

    /// Adds `opened`, whose turns are recorded as `job` says. Checked,
    /// recorded, and added under the lock the stop drains the sessions
    /// under, so a session opened while the daemon stops is shut down here,
    /// neither left running nor recorded. Its saved conversation is replayed
    /// before that lock, which every request takes.
    async fn add(
        &self,
        job: SessionJobRecord,
        opened: OpenedSession,
    ) -> Result<Arc<LoadedSession>, RpcError> {
        let session = opened.runtime.session_id().clone();
        let auths = options::available_auths(opened.runtime.credentials()).await;
        let entry = LoadedSession::new(
            job.job(),
            Arc::clone(&self.jobs),
            opened,
            &auths,
            &self.wakes,
            &self.settings,
        );
        let refused = {
            let mut loaded = self.loaded();
            match self
                .refuse_when_stopping()
                .and_then(|()| job.claim(&self.jobs, &session))
            {
                Ok(()) => {
                    let entry = Arc::new(entry);
                    loaded.insert(session, Arc::clone(&entry));
                    return Ok(entry);
                }
                Err(error) => error,
            }
        };
        // Never added, so nothing else holds the runtime or queues on it.
        if let Some(runtime) =
            Arc::into_inner(entry.runtime).and_then(tokio::sync::Mutex::into_inner)
        {
            runtime.shutdown().await;
        }
        Err(refused)
    }

    /// Cancels a session's turn, if one is under way, even one that has not
    /// begun its prompt yet.
    pub(super) async fn cancel(
        &self,
        params: SessionCancelParams,
    ) -> Result<SessionCancelResponse, RpcError> {
        if let Some(session) = self.loaded_session(&params.session).await? {
            session.cancel_turn();
        }
        Ok(SessionCancelResponse {})
    }

    /// Hands a client's answer to one of a session's open input requests
    /// (#1660). The first answer wins: a later one finds the request gone.
    pub(super) async fn answer(
        &self,
        params: SessionAnswerParams,
    ) -> Result<SessionAnswerResponse, RpcError> {
        let Some(session) = self.loaded_session(&params.session).await? else {
            return Err(inputs::not_open(params.request));
        };
        session.change_inputs(|inputs| inputs.answer(params.request, params.answer))?;
        Ok(SessionAnswerResponse {})
    }

    /// Stops one of a session's open input requests from timing out, for a
    /// client whose user started on it (#1660).
    pub(super) async fn hold(
        &self,
        params: SessionHoldParams,
    ) -> Result<SessionHoldResponse, RpcError> {
        let Some(session) = self.loaded_session(&params.session).await? else {
            return Err(inputs::not_open(params.request));
        };
        session.change_inputs(|inputs| inputs.hold(params.request))?;
        Ok(SessionHoldResponse {})
    }

    /// Subscribes a client to a session's events, resuming after its cursor
    /// when the session's feed still can.
    pub(super) async fn attach(
        &self,
        params: SessionAttachParams,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        self.refuse_when_stopping()?;
        // Taken under the turn lock a prompt starts its turn under, so an
        // attach never sees the last turn's end once the job list says the
        // next turn runs, and the idle unload never takes a session a client
        // just attached to.
        // Applied only if this attach reopens the session: one the daemon
        // has loaded keeps the environment and setup it opened with.
        // Its grant is its environment's: an attach names none.
        let with = OpenWith::under(
            grant::asked(/*named*/ None, &params.environment)?,
            &params.environment,
            params.setup,
        )?;
        let (session, attached) = self
            .enter_session(&params.session, &with, |session, _turn| {
                Ok(session.feed.attach(params.after.as_ref()))
            })
            .await?;
        Ok(attach_response(&session.workspace, attached))
    }

    /// Stops every session for the daemon's exit, beginning the stop if
    /// [`Self::begin_stop`] has not. Stops each turn, waits until
    /// [`DRAIN_TIMEOUT`] after `stop_requested` for the turns to save and
    /// the runtimes to tear down, the idle unload's included, and records
    /// every job still under way as interrupted. Returns whether every turn
    /// saved and every runtime was torn down in time.
    pub(super) async fn stop(&self, stop_requested: tokio::time::Instant) -> bool {
        self.begin_stop(DetachReason::DaemonStopping);
        let sessions: Vec<_> = self.loaded().drain().map(|(_, session)| session).collect();
        for session in &sessions {
            // The turn stops as one its client lost, so the session keeps
            // what a later daemon resumes (#1661).
            session.handle.close();
        }
        let teardowns = sessions.iter().map(|session| async {
            // Free once the session's turn saved and recorded its end.
            let runtime = session.runtime.lock().await.take();
            if let Some(runtime) = runtime {
                runtime.shutdown().await;
            }
        });
        let drained = tokio::time::timeout_at(
            stop_requested + DRAIN_TIMEOUT,
            futures_util::future::join(
                futures_util::future::join_all(teardowns),
                // The unload decides under the lock `begin_stop` took, so a
                // session it took from `loaded` is torn down under this.
                self.unloading.write(),
            ),
        )
        .await;
        if drained.is_err() {
            tracing::warn!(
                timeout = ?DRAIN_TIMEOUT,
                "turns did not save within DRAIN_TIMEOUT of the stop request; exiting anyway"
            );
        }
        self.jobs.close();
        drained.is_ok()
    }

    fn refuse_when_stopping(&self) -> Result<(), RpcError> {
        if self.stopping.is_cancelled() {
            Err(stopping())
        } else {
            Ok(())
        }
    }

    /// Takes `session`, loaded as `id`, out of `loaded` and marks it
    /// unloaded, unless the daemon is stopping or `id` now names another
    /// session; whether it did. `turn` is `session`'s turn lock, held, so a
    /// request either used the session before this or finds it gone and
    /// looks it up again. Under the lock `begin_stop` takes, so a stop either
    /// drains the session first or finds it gone. Lock order: the turn
    /// lock, then `loaded`.
    fn evict(&self, id: &SessionId, session: &Arc<LoadedSession>, turn: &mut TurnSlot) -> bool {
        let mut loaded = self.loaded();
        let current = loaded
            .get(id)
            .is_some_and(|loaded| Arc::ptr_eq(loaded, session));
        if self.stopping.is_cancelled() || !current {
            return false;
        }
        loaded.remove(id);
        turn.residency = Residency::Unloaded;
        true
    }

    fn loaded(&self) -> MutexGuard<'_, HashMap<SessionId, Arc<LoadedSession>>> {
        // The map is only inserted into, removed from, and drained, so a
        // panic cannot leave it half-changed.
        self.loaded.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The answer to an attach or observe of the session in `workspace`, and
/// the subscription its events then arrive on.
fn attach_response(workspace: &Path, attached: Attached) -> (SessionAttachResponse, Subscription) {
    let Attached {
        stream,
        replay,
        replay_events,
        job,
        turn,
        inputs,
        title,
        options,
        subscription,
    } = attached;
    let response = SessionAttachResponse {
        stream,
        workspace: workspace.to_path_buf(),
        replay,
        replay_events,
        state: job,
        turn,
        inputs,
        title,
        options,
    };
    (response, subscription)
}

/// How a session's turns are recorded once it is added.
enum SessionJobRecord {
    /// On a new job, registered as the session is added.
    Register(JobRecord),
    /// On the session's last job, which an earlier daemon or an earlier load
    /// recorded.
    Reuse(JobRecord),
}

impl SessionJobRecord {
    fn job(&self) -> &JobRecord {
        match self {
            Self::Register(record) | Self::Reuse(record) => record,
        }
    }

    /// Records the job in `jobs` as the one of `session`'s turns, refusing
    /// it when the registry no longer agrees, as when another job began
    /// writing the session while it reopened (#1661).
    fn claim(self, jobs: &JobRegistry, session: &SessionId) -> Result<(), RpcError> {
        match (self, jobs.session_job(session)) {
            (Self::Register(record), SessionJob::New) => {
                jobs.register(record).map_err(|error| match error {
                    // Stopping, or restarting for an upgrade.
                    RegisterError::Closed => stopping(),
                    RegisterError::Io(error) => RpcError::new(
                        ErrorCode::InternalError,
                        format!("could not record the session's job: {error}"),
                    ),
                })
            }
            (Self::Reuse(record), SessionJob::Last(last)) if last.id == record.id => {
                // The session reopens with the grant its environment
                // carries, so its record says the same.
                jobs.set_grant(&record.id, record.grant);
                Ok(())
            }
            (_, SessionJob::Busy(job)) => Err(writing(&job, session)),
            (Self::Register(_) | Self::Reuse(_), SessionJob::New | SessionJob::Last(_)) => {
                Err(RpcError::new(
                    ErrorCode::SessionBusy,
                    format!("session {session}'s job changed while the daemon opened it"),
                ))
            }
        }
    }
}

/// The error for a session job `job` writes, so the daemon must not open it
/// a second time.
fn writing(job: &JobId, session: &SessionId) -> RpcError {
    RpcError::new(
        ErrorCode::SessionBusy,
        format!("job {job} is writing session {session}"),
    )
}

/// The error for a session the daemon could not open: `SignInRequired`
/// when signing in can fix it, in Rho's words with the full cause only in
/// the daemon's log, else `SessionOpenFailed`.
fn open_failed(error: anyhow::Error) -> RpcError {
    match CredentialFailure::find(&error) {
        Some(failure) => {
            tracing::warn!(error = format!("{error:#}"), "session needs a sign-in");
            RpcError::new(
                ErrorCode::SignInRequired,
                format!("could not open session: {}", failure.message()),
            )
        }
        None => RpcError::new(
            ErrorCode::SessionOpenFailed,
            format!("could not open session: {error:#}"),
        ),
    }
}

/// The error a request that needs the session answers while a delete
/// unloads it.
fn being_deleted() -> RpcError {
    RpcError::new(ErrorCode::SessionBusy, "the session is being deleted")
}

fn stopping() -> RpcError {
    RpcError::new(ErrorCode::DaemonStopping, "the daemon is stopping")
}

/// The error a prompt answers when its turn ended, as `outcome` says,
/// before it began: `TurnEnded`, with the outcome as its data.
fn ended_early(outcome: &TurnOutcome) -> RpcError {
    let message = match outcome {
        TurnOutcome::Failed { error } => error.message.clone(),
        TurnOutcome::Cancelled => "the turn was cancelled before it started".to_owned(),
        TurnOutcome::TimedOut { timeout_ms } => {
            format!("the turn reached its {timeout_ms}ms time limit before it started")
        }
        // Only those three end a turn before it begins.
        _ => "the turn ended before it started".to_owned(),
    };
    let data = serde_json::to_value(outcome).expect("a turn outcome serializes");
    RpcError::new(ErrorCode::TurnEnded, message).with_data(data)
}

#[cfg(test)]
#[path = "open_failed_tests.rs"]
mod open_failed_tests;
