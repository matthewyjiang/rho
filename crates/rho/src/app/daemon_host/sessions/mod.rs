//! The sessions the daemon hosts, prompted turn by turn, whose turns run in
//! the daemon and outlive the client that sent them.
//!
//! A loaded session owns its [`SessionRuntime`] and the [`SessionHandle`]
//! that cancels and steers it without waiting for the turn, and its turns
//! are one job in the registry. Requests name the session by its session
//! id. A prompt or an attach for a saved session the daemon has not loaded,
//! such as one an earlier daemon ran or one the TUI saved, reopens it from
//! its file in its own workspace, recording its turns on the session's last
//! job (#1661). Cancel, steer, answer, and hold never load a session: one
//! that is not loaded has no turn to act on.
//!
//! Clients attach to a loaded session to follow its events (#1657); see
//! [`feed`].
//!
//! Once the daemon starts stopping, it refuses new sessions and prompts and
//! detaches every client; then it stops the turns under way, waits for them
//! to save, and records them interrupted.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::Duration,
};

use rho_protocol::{
    DetachReason, ErrorCode, RpcError, SessionAnswerParams, SessionAnswerResponse,
    SessionAttachParams, SessionAttachResponse, SessionCancelParams, SessionCancelResponse,
    SessionHoldParams, SessionHoldResponse, SessionNewParams, SessionNewResponse,
    SessionPromptParams, SessionPromptResponse, SessionSteerParams, SessionSteerResponse,
    Subscription, TurnOutcome,
};
use rho_sdk::{CancellationToken, SessionId, UserInput};

use super::{
    jobs::{
        JobEvent, JobId, JobRecord, JobRegistry, JobState, SessionJob, StartTurnError, TurnEnd,
    },
    subagents::Subagents,
    wire::{session_to_wire, state_event},
};
use crate::{
    app::{
        session_events::LiveEvents,
        session_runtime::{Closing, Logouts, SessionHandle, SessionRuntime, SteerError},
    },
    config::Config,
};

mod feed;
mod inputs;
mod open;
mod reopen;
#[cfg(test)]
mod test_events;
mod turn;

use feed::{Attached, Feed};
use inputs::Inputs;
use open::{open_session, OpenedSession, Opening, Workspace};
use reopen::ReopenLocks;
use turn::{PromptStart, TurnLimit, TurnTask};

/// How long the daemon's stop waits for its turns to save once it was
/// asked to stop, before it records them interrupted and exits anyway.
//
// Tripwire, not measured: a cancelled turn saves in milliseconds, so this
// fires only on a turn stuck in a tool that ignores the cancel. It runs from
// the stop request and stays under `DAEMON_STOP_TIMEOUT` (5 s) with room for
// the rest of the exit (44 ms max measured), so `rho daemon stop` still sees
// the daemon go. Draining that waits for turns to finish is #1713's to size.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(4);

/// Every session the daemon has loaded, and the job registry that records
/// their turns.
pub(super) struct Sessions {
    jobs: Arc<JobRegistry>,
    loaded: Mutex<HashMap<SessionId, Arc<LoadedSession>>>,
    /// The saved sessions being reopened, one at a time each.
    reopening: ReopenLocks,
    /// The daemon-wide budget the sessions' subagents share, and their jobs.
    subagents: Subagents,
    /// The daemon's logouts, which every session's credentials follow.
    logouts: Logouts,
    /// Cancelled once the daemon starts stopping ([`Self::begin_stop`]).
    stopping: CancellationToken,
}

/// A session this daemon loaded.
struct LoadedSession {
    /// The job that records the session's turns, in `jobs`.
    job: JobId,
    jobs: Arc<JobRegistry>,
    /// Held by the turn under way; taken when the daemon stops.
    runtime: tokio::sync::Mutex<Option<SessionRuntime>>,
    handle: SessionHandle,
    /// The config the session was opened with, which each turn reads.
    config: Config,
    /// The directory the session works in.
    workspace: PathBuf,
    /// The cancel of the session's current or last turn. A prompt replaces
    /// it as its turn starts, and a cancel trips it and the session's
    /// handle, both under this lock, so a cancel never reaches a turn it did
    /// not see start.
    turn_cancel: Mutex<CancellationToken>,
    /// The session's events, as attached clients follow them.
    feed: Feed,
    /// Turns the runtime's events into the feed's; only the turn under way
    /// uses it, so the lock is never contended.
    live: Mutex<LiveEvents>,
    /// The input requests the turn under way has open (#1660).
    inputs: Mutex<Inputs>,
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

impl LoadedSession {
    /// A session opened as `opened`, whose turns are recorded on `job` in
    /// `jobs`: a new job for a new session, the last one for a reopened one.
    /// Its feed begins with the conversation the session has saved and how
    /// the job's last turn ended, which a new session has neither of, so a
    /// client attaching to a reopened session learns that end (#1661).
    fn new(job: &JobRecord, jobs: Arc<JobRegistry>, opened: OpenedSession) -> Self {
        let OpenedSession { runtime, config } = opened;
        let mut live = LiveEvents::new(runtime.workspace_root(), runtime.tools());
        let feed = Feed::new((&job.state).into());
        if let Some(saved) = saved_replay(&mut live, &runtime, &job.id) {
            feed.end_turn(saved);
        }
        if let Some(outcome) = job.outcome.clone() {
            feed.publish([
                rho_protocol::SessionEvent::TurnEnded { outcome },
                state_event(&job.state),
            ]);
        }
        let job = job.id.clone();
        Self {
            handle: runtime.handle(),
            workspace: runtime.workspace_root().to_path_buf(),
            live: Mutex::new(live),
            feed,
            job,
            jobs,
            runtime: tokio::sync::Mutex::new(Some(runtime)),
            inputs: Mutex::new(Inputs::new(
                config
                    .questionnaire
                    .timeout_seconds
                    .map(|seconds| Duration::from_secs(seconds.get())),
            )),
            config,
            turn_cancel: Mutex::default(),
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

    /// Applies `event` to the session's job and tells attached clients the
    /// state it left the job in.
    fn record(&self, event: JobEvent) {
        match self.jobs.record(&self.job, event) {
            Ok(state) => self.feed.publish([state_event(&state)]),
            Err(error) => {
                tracing::warn!(job = %self.job, %error, "could not record a turn's progress");
            }
        }
    }

    /// Records that the turn ended as `end`, keeping `outcome` on the job for
    /// a later daemon to replay, and tells attached clients `outcome`, if
    /// any, then the state it left the job in, in one publish, so no
    /// client's replay holds one without the other (#1669).
    fn end_turn(&self, outcome: Option<TurnOutcome>, end: TurnEnd) {
        let ended = outcome
            .clone()
            .map(|outcome| rho_protocol::SessionEvent::TurnEnded { outcome });
        match self.jobs.end_turn(&self.job, end, outcome) {
            Ok(state) => self
                .feed
                .publish(ended.into_iter().chain([state_event(&state)])),
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

    fn turn_cancel(&self) -> MutexGuard<'_, CancellationToken> {
        // Only ever replaced or cancelled whole, so a panic cannot leave it
        // half-changed.
        self.turn_cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Cancels the turn under way, if any, even one that has not begun its
    /// prompt yet.
    fn cancel_turn(&self) {
        // Both under the `turn_cancel` lock, which a prompt takes to start
        // its turn, so the handle's cancel cannot reach a turn started after
        // this cancel. Lock order: `turn_cancel`, then the prompt gate's
        // slot inside `handle.cancel()`; nothing takes them the other way.
        let turn_cancel = self.turn_cancel();
        turn_cancel.cancel();
        self.handle.cancel();
    }
}

impl Sessions {
    /// No sessions yet; their turns are recorded in `jobs`.
    pub(super) fn new(jobs: Arc<JobRegistry>) -> Self {
        let stopping = CancellationToken::new();
        Self {
            subagents: Subagents::new(Arc::clone(&jobs), stopping.clone()),
            jobs,
            loaded: Mutex::default(),
            reopening: ReopenLocks::default(),
            logouts: Logouts::default(),
            stopping,
        }
    }

    /// The sessions' subagents: the daemon-wide budget and its queue.
    pub(super) fn subagents(&self) -> &Subagents {
        &self.subagents
    }

    /// Starts the daemon's stop: from now on no session opens, no turn
    /// starts, no client attaches, and every client attached to a session,
    /// even by an attach that raced the stop, is detached as
    /// `daemon_stopping`. Call before the transport closes the connections,
    /// so each client learns why. Calling again does nothing.
    pub(super) fn begin_stop(&self) {
        // Under the lock `add` checks `stopping` under, so no session is
        // added after this with its feed open.
        let loaded = self.loaded();
        self.stopping.cancel();
        for session in loaded.values() {
            session.feed.close(DetachReason::DaemonStopping);
        }
    }

    /// Opens a new session in `cwd` and registers the job of its turns.
    pub(super) async fn open_new(
        &self,
        params: SessionNewParams,
    ) -> Result<SessionNewResponse, RpcError> {
        self.refuse_when_stopping()?;
        let workspace = Workspace::parse(&params.cwd)?;
        let job = JobId::generate();
        let opened = open_session(
            Opening::New(workspace),
            &self.logouts,
            &self.subagents,
            &job,
        )
        .await
        .map_err(open_failed)?;
        let session = opened.runtime.session_id().clone();
        let record = JobRecord::new_session(
            job.clone(),
            opened.runtime.workspace_root().to_path_buf(),
            session.clone(),
        );
        self.add(SessionJobRecord::Register(record), opened).await?;
        tracing::info!(%session, %job, "opened session");
        Ok(SessionNewResponse {
            session: session_to_wire(session),
        })
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
        let entry = LoadedSession::new(job.job(), Arc::clone(&self.jobs), opened);
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
        if let Some(runtime) = entry.runtime.into_inner() {
            runtime.shutdown().await;
        }
        Err(refused)
    }

    /// Starts a turn of `text` on a session and returns once the turn saved
    /// its prompt. The turn runs in a daemon task from then on.
    pub(super) async fn prompt(
        &self,
        params: SessionPromptParams,
    ) -> Result<SessionPromptResponse, RpcError> {
        self.refuse_when_stopping()?;
        let session = self.session(&params.session).await?;
        let cancel = {
            let mut turn_cancel = session.turn_cancel();
            match self.jobs.start_turn(&session.job) {
                // Told under the `turn_cancel` lock, which the last turn's
                // end is told under too, so the feed never says `running`
                // before the last turn's end, nor a turn's end after the next
                // turn began.
                Ok(()) => session.feed.publish([state_event(&JobState::Running)]),
                Err(StartTurnError::Busy) => {
                    return Err(RpcError::new(
                        ErrorCode::SessionBusy,
                        "session already has a turn in progress",
                    ))
                }
                Err(StartTurnError::Unknown(_)) => {
                    return Err(RpcError::new(
                        ErrorCode::InternalError,
                        format!("session's job {} is not recorded", session.job),
                    ))
                }
            }
            *turn_cancel = CancellationToken::new();
            turn_cancel.clone()
        };
        // Spawned before anything awaits, so the turn runs and records its
        // end apart from this request: the transport joins every request it
        // read, but this one stops waiting once the daemon stops.
        let start = TurnTask {
            session,
            stopping: self.stopping.clone(),
            cancel,
            limit: params.timeout_ms.map(TurnLimit::starting_now),
        }
        .spawn(params.text);
        // An answer that is already in wins over the stop, so a prompt the
        // turn saved is not reported refused. Once the daemon stops, the
        // turn goes on until the stop drains it.
        tokio::select! {
            biased;
            start = start => match start {
                Ok(PromptStart::Saved) => Ok(SessionPromptResponse {}),
                Ok(PromptStart::Ended(outcome)) => Err(ended_early(&outcome)),
                // Only a turn the daemon's stop interrupted, or one whose
                // task the exiting runtime dropped, sends no answer.
                Err(_) => Err(stopping()),
            },
            () = self.stopping.cancelled() => Err(stopping()),
        }
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

    /// Stages `text` for the running turn's next model request. The receipt
    /// is dropped: the turn reports what it applied with its events.
    pub(super) async fn steer(
        &self,
        params: SessionSteerParams,
    ) -> Result<SessionSteerResponse, RpcError> {
        let not_steerable =
            || RpcError::new(ErrorCode::NotSteerable, "no model request is taking input");
        let Some(session) = self.loaded_session(&params.session).await? else {
            return Err(not_steerable());
        };
        match session.handle.steer(UserInput::text(params.text)) {
            Ok(_receipt) => Ok(SessionSteerResponse {}),
            Err(SteerError::NotRunning) => Err(not_steerable()),
            Err(SteerError::Refused(error)) => Err(RpcError::new(
                ErrorCode::SteerRefused,
                format!("could not steer session: {error}"),
            )),
        }
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
        let session = self.session(&params.session).await?;
        // Taken under the `turn_cancel` lock a prompt starts its turn under,
        // so an attach never sees the last turn's end once the job list says
        // the next turn runs.
        let Attached {
            stream,
            replay,
            replay_events,
            job,
            inputs,
            subscription,
        } = {
            let _turn = session.turn_cancel();
            session.feed.attach(params.after.as_ref())
        };
        let response = SessionAttachResponse {
            stream,
            workspace: session.workspace.clone(),
            replay,
            replay_events,
            state: job,
            inputs,
        };
        Ok((response, subscription))
    }

    /// Stops every session for the daemon's exit, beginning the stop if
    /// [`Self::begin_stop`] has not. Stops each turn, waits until
    /// [`DRAIN_TIMEOUT`] after `stop_requested` for the turns to save and
    /// the runtimes to tear down, and records every job still under way as
    /// interrupted.
    pub(super) async fn stop(&self, stop_requested: tokio::time::Instant) {
        self.begin_stop();
        let sessions: Vec<_> = self.loaded().drain().map(|(_, session)| session).collect();
        for session in &sessions {
            // The turn stops as one its client lost, so the session keeps
            // what a later daemon resumes (#1661).
            session.handle.close(Closing::Disconnected);
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
            futures_util::future::join_all(teardowns),
        )
        .await;
        if drained.is_err() {
            tracing::warn!(
                timeout = ?DRAIN_TIMEOUT,
                "turns did not save within DRAIN_TIMEOUT of the stop request; exiting anyway"
            );
        }
        self.jobs.close();
    }

    fn refuse_when_stopping(&self) -> Result<(), RpcError> {
        if self.stopping.is_cancelled() {
            Err(stopping())
        } else {
            Ok(())
        }
    }

    fn loaded(&self) -> MutexGuard<'_, HashMap<SessionId, Arc<LoadedSession>>> {
        // The map is only inserted into, removed from, and drained, so a
        // panic cannot leave it half-changed.
        self.loaded.lock().unwrap_or_else(PoisonError::into_inner)
    }
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
            (Self::Register(record), SessionJob::New) => jobs.register(record).map_err(|error| {
                RpcError::new(
                    ErrorCode::InternalError,
                    format!("could not record the session's job: {error}"),
                )
            }),
            (Self::Reuse(record), SessionJob::Last(last)) if last.id == record.id => Ok(()),
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

fn open_failed(error: anyhow::Error) -> RpcError {
    RpcError::new(
        ErrorCode::SessionOpenFailed,
        format!("could not open session: {error:#}"),
    )
}

fn stopping() -> RpcError {
    RpcError::new(ErrorCode::DaemonStopping, "the daemon is stopping")
}

/// The error a prompt answers when its turn ended, as `outcome` says,
/// before it saved the prompt: `TurnEnded`, with the outcome as its data.
fn ended_early(outcome: &TurnOutcome) -> RpcError {
    let message = match outcome {
        TurnOutcome::Failed { error } => error.message.clone(),
        TurnOutcome::Cancelled => "the turn was cancelled before it started".to_owned(),
        TurnOutcome::TimedOut { timeout_ms } => {
            format!("the turn reached its {timeout_ms}ms time limit before it started")
        }
        // Only those three end a turn before its prompt is saved.
        _ => "the turn ended before it started".to_owned(),
    };
    let data = serde_json::to_value(outcome).expect("a turn outcome serializes");
    RpcError::new(ErrorCode::TurnEnded, message).with_data(data)
}
