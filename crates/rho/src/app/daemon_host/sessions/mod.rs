//! The sessions the daemon hosts, prompted turn by turn, whose turns run in
//! the daemon and outlive the client that sent them.
//!
//! A loaded session owns its [`SessionRuntime`] and the [`SessionHandle`]
//! that cancels and steers it without waiting for the turn, and its turns
//! are one job in the registry. Requests name the session by its session
//! id. A session this daemon has not loaded, such as one an earlier daemon
//! ran or one the TUI saved, is refused until reopening lands (#1661).
//!
//! Clients attach to a loaded session to follow its events (#1657); see
//! [`feed`].
//!
//! Once the daemon starts stopping, it refuses new sessions and prompts and
//! detaches every client; then it stops the turns under way, waits for them
//! to save, and records them interrupted.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use rho_protocol::{
    DetachReason, ErrorCode, RpcError, SessionAttachParams, SessionAttachResponse,
    SessionCancelParams, SessionCancelResponse, SessionNewParams, SessionNewResponse,
    SessionPromptParams, SessionPromptResponse, SessionSteerParams, SessionSteerResponse,
    Subscription,
};
use rho_sdk::{CancellationToken, SessionId, UserInput};

use super::{
    jobs::{JobId, JobRecord, JobRegistry, StartTurnError},
    subagents::Subagents,
    wire::{session_from_wire, session_to_wire},
};
use crate::{
    app::{
        session_events::LiveEvents,
        session_runtime::{Closing, Logouts, SessionHandle, SessionRuntime, SteerError},
    },
    config::Config,
};

mod feed;
mod open;
mod turn;

use feed::{Attached, Feed};
use open::{open_session, OpenedSession, Workspace};
use turn::{PromptStart, TurnTask};

/// How long the daemon's stop waits for its turns to save once it was
/// asked to stop, before it records them interrupted and exits anyway.
//
// Tripwire, not measured: a cancelled turn saves in milliseconds, so this
// fires only on a turn stuck in a tool that ignores the cancel. It runs from
// the stop request and stays under `DAEMON_STOP_TIMEOUT` (5 s) with room for
// the rest of the exit (44 ms max measured), so `rho daemon stop` still sees
// the daemon go. Draining that waits for turns to finish is #1661's to size.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(4);

/// Every session the daemon has loaded, and the job registry that records
/// their turns.
pub(super) struct Sessions {
    jobs: Arc<JobRegistry>,
    loaded: Mutex<HashMap<SessionId, Arc<LoadedSession>>>,
    /// The daemon-wide budget the sessions' subagents share, and their jobs.
    subagents: Subagents,
    /// The daemon's logouts, which every session's credentials follow.
    logouts: Logouts,
    /// Cancelled once the daemon starts stopping ([`Self::begin_stop`]).
    stopping: CancellationToken,
}

/// A session this daemon loaded.
struct LoadedSession {
    /// The job that records the session's turns.
    job: JobId,
    /// Held by the turn under way; taken when the daemon stops.
    runtime: tokio::sync::Mutex<Option<SessionRuntime>>,
    handle: SessionHandle,
    /// The config the session was opened with, which each turn reads.
    config: Config,
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
}

impl LoadedSession {
    /// A session opened new: nothing saved yet, so its feed starts empty.
    fn new(job: JobId, runtime: SessionRuntime, config: Config) -> Self {
        Self {
            job,
            handle: runtime.handle(),
            live: Mutex::new(LiveEvents::new(runtime.workspace_root(), runtime.tools())),
            feed: Feed::new(),
            runtime: tokio::sync::Mutex::new(Some(runtime)),
            config,
            turn_cancel: Mutex::default(),
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
}

impl Sessions {
    /// No sessions yet; their turns are recorded in `jobs`.
    pub(super) fn new(jobs: Arc<JobRegistry>) -> Self {
        let stopping = CancellationToken::new();
        Self {
            subagents: Subagents::new(Arc::clone(&jobs), stopping.clone()),
            jobs,
            loaded: Mutex::default(),
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
        // Under the lock `open_new` checks `stopping` under, so no session
        // is added after this with its feed open.
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
        let OpenedSession { runtime, config } =
            open_session(&workspace, &self.logouts, &self.subagents, &job)
                .await
                .map_err(|error| {
                    RpcError::new(
                        ErrorCode::SessionOpenFailed,
                        format!("could not open session: {error:#}"),
                    )
                })?;
        let session = runtime.session_id().clone();
        let record = JobRecord::new_session(
            job.clone(),
            runtime.workspace_root().to_path_buf(),
            session.clone(),
        );
        // Checked, recorded, and added under the lock the stop drains the
        // sessions under, so a session opened while the daemon stops is
        // closed here, neither left running nor recorded.
        let refused = {
            let mut loaded = self.loaded();
            if self.stopping.is_cancelled() {
                Some((runtime, stopping()))
            } else if let Err(error) = self.jobs.register(record) {
                let error = RpcError::new(
                    ErrorCode::InternalError,
                    format!("could not record the session's job: {error}"),
                );
                Some((runtime, error))
            } else {
                let entry = LoadedSession::new(job.clone(), runtime, config);
                loaded.insert(session.clone(), Arc::new(entry));
                None
            }
        };
        if let Some((runtime, error)) = refused {
            runtime.shutdown().await;
            return Err(error);
        }
        tracing::info!(%session, %job, "opened session");
        Ok(SessionNewResponse {
            session: session_to_wire(session),
        })
    }

    /// Starts a turn of `text` on a session and returns once the turn saved
    /// its prompt. The turn runs in a daemon task from then on.
    pub(super) async fn prompt(
        &self,
        params: SessionPromptParams,
    ) -> Result<SessionPromptResponse, RpcError> {
        self.refuse_when_stopping()?;
        let session = self.loaded_session(&params.session)?;
        let cancel = {
            let mut turn_cancel = session.turn_cancel();
            match self.jobs.start_turn(&session.job) {
                Ok(()) => {}
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
            jobs: Arc::clone(&self.jobs),
            session,
            stopping: self.stopping.clone(),
            cancel,
        }
        .spawn(params.text);
        // An answer that is already in wins over the stop, so a prompt the
        // turn saved is not reported refused. Once the daemon stops, the
        // turn goes on until the stop drains it.
        tokio::select! {
            biased;
            start = start => match start {
                Ok(PromptStart::Saved) => Ok(SessionPromptResponse {}),
                Ok(PromptStart::Cancelled) => Err(RpcError::new(
                    ErrorCode::TurnCancelled,
                    "the turn was cancelled before it started",
                )),
                Ok(PromptStart::Failed(error)) => Err(RpcError::new(ErrorCode::TurnFailed, error)),
                // Only a turn the daemon's stop interrupted, or one whose
                // task the exiting runtime dropped, sends no answer.
                Err(_) => Err(stopping()),
            },
            () = self.stopping.cancelled() => Err(stopping()),
        }
    }

    /// Cancels a session's turn, if one is under way, even one that has not
    /// begun its prompt yet.
    pub(super) fn cancel(
        &self,
        params: SessionCancelParams,
    ) -> Result<SessionCancelResponse, RpcError> {
        let session = self.loaded_session(&params.session)?;
        // Both under the `turn_cancel` lock, which a prompt takes to start
        // its turn, so the handle's cancel cannot reach a turn started after
        // this cancel. Lock order: `turn_cancel`, then the prompt gate's
        // slot inside `handle.cancel()`; nothing takes them the other way.
        let turn_cancel = session.turn_cancel();
        turn_cancel.cancel();
        session.handle.cancel();
        drop(turn_cancel);
        Ok(SessionCancelResponse {})
    }

    /// Stages `text` for the running turn's next model request. The receipt
    /// is dropped: the turn reports what it applied with its events.
    pub(super) fn steer(
        &self,
        params: SessionSteerParams,
    ) -> Result<SessionSteerResponse, RpcError> {
        let session = self.loaded_session(&params.session)?;
        match session.handle.steer(UserInput::text(params.text)) {
            Ok(_receipt) => Ok(SessionSteerResponse {}),
            Err(SteerError::NotRunning) => Err(RpcError::new(
                ErrorCode::NotSteerable,
                "no model request is taking input",
            )),
            Err(SteerError::Refused(error)) => Err(RpcError::new(
                ErrorCode::SteerRefused,
                format!("could not steer session: {error}"),
            )),
        }
    }

    /// Subscribes a client to a session's events, resuming after its cursor
    /// when the session's feed still can.
    pub(super) fn attach(
        &self,
        params: SessionAttachParams,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        self.refuse_when_stopping()?;
        let session = self.loaded_session(&params.session)?;
        let Attached {
            stream,
            replay,
            job,
            subscription,
        } = session.feed.attach(params.after.as_ref());
        let response = SessionAttachResponse {
            stream,
            replay,
            state: job,
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

    /// The loaded session `id` names. A well-formed id this daemon has not
    /// loaded is `SessionNotLoaded`; whether such a session exists at all
    /// is told apart once the daemon can reopen sessions (#1661).
    fn loaded_session(&self, id: &rho_protocol::SessionId) -> Result<Arc<LoadedSession>, RpcError> {
        let id = session_from_wire(id)
            .map_err(|_| RpcError::new(ErrorCode::InvalidParams, "not a session id"))?;
        self.loaded().get(&id).cloned().ok_or_else(|| {
            RpcError::new(
                ErrorCode::SessionNotLoaded,
                "session is not loaded in this daemon",
            )
        })
    }

    fn refuse_when_stopping(&self) -> Result<(), RpcError> {
        if self.stopping.is_cancelled() {
            Err(stopping())
        } else {
            Ok(())
        }
    }

    fn loaded(&self) -> MutexGuard<'_, HashMap<SessionId, Arc<LoadedSession>>> {
        // The map is only inserted into and drained, so a panic cannot leave
        // it half-changed.
        self.loaded.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn stopping() -> RpcError {
    RpcError::new(ErrorCode::DaemonStopping, "the daemon is stopping")
}
