//! The threads the daemon hosts: one session each, prompted turn by turn,
//! whose turns run in the daemon and outlive the client that sent them.
//!
//! A live thread owns its [`SessionRuntime`] and the [`SessionHandle`] that
//! cancels and steers it without waiting for the turn. A thread loaded from
//! an earlier daemon's record has no runtime here: it is listed, but
//! prompting it is refused until reopening lands (#1661).
//!
//! Once the daemon stops, it refuses new threads and prompts, cancels the
//! turns under way, waits for them to save, and records them interrupted.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use rho_protocol::{
    ErrorCode, RpcError, ThreadCancelParams, ThreadCancelResponse, ThreadCreateParams,
    ThreadCreateResponse, ThreadPromptParams, ThreadPromptResponse, ThreadSteerParams,
    ThreadSteerResponse,
};
use rho_sdk::{CancellationToken, UserInput};

use super::jobs::{JobId, JobRecord, JobRegistry, StartTurnError};
use crate::{
    app::session_runtime::{Closing, Logouts, SessionHandle, SessionRuntime, SteerError},
    config::Config,
};

mod open;
mod turn;

use open::{open_thread, OpenedThread, ThreadWorkspace};
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

/// Every thread the daemon runs, and the job registry that records them.
pub(super) struct Threads {
    jobs: Arc<JobRegistry>,
    live: Mutex<HashMap<JobId, Arc<LiveThread>>>,
    /// The daemon's logouts, which every thread's credentials follow.
    logouts: Logouts,
    /// Cancelled once the daemon stops.
    stopping: CancellationToken,
}

/// A thread this daemon opened.
struct LiveThread {
    /// Held by the turn under way; taken when the daemon stops.
    runtime: tokio::sync::Mutex<Option<SessionRuntime>>,
    handle: SessionHandle,
    /// The config the thread was opened with, which each turn reads.
    config: Config,
    /// The cancel of the thread's current or last turn. A prompt replaces it
    /// as its turn starts, under this lock, so a cancel never trips the
    /// token of a turn it did not see start.
    turn_cancel: Mutex<CancellationToken>,
}

impl LiveThread {
    fn new(runtime: SessionRuntime, config: Config) -> Self {
        Self {
            handle: runtime.handle(),
            runtime: tokio::sync::Mutex::new(Some(runtime)),
            config,
            turn_cancel: Mutex::default(),
        }
    }

    fn turn_cancel(&self) -> MutexGuard<'_, CancellationToken> {
        // Only ever replaced or cancelled whole, so a panic cannot leave it
        // half-changed.
        self.turn_cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Threads {
    /// The threads of a daemon whose stop cancels `stopping`.
    pub(super) fn new(jobs: Arc<JobRegistry>, stopping: CancellationToken) -> Self {
        Self {
            jobs,
            live: Mutex::default(),
            logouts: Logouts::default(),
            stopping,
        }
    }

    /// Opens a new thread on a new session in `cwd`.
    pub(super) async fn create(
        &self,
        params: ThreadCreateParams,
    ) -> Result<ThreadCreateResponse, RpcError> {
        self.refuse_when_stopping()?;
        let workspace = ThreadWorkspace::parse(&params.cwd)?;
        let OpenedThread { runtime, config } = open_thread(&workspace, &self.logouts)
            .await
            .map_err(|error| {
                RpcError::new(
                    ErrorCode::ThreadOpenFailed,
                    format!("could not open thread: {error:#}"),
                )
            })?;
        let session_id = runtime.session_id().clone();
        let record =
            JobRecord::new_thread(runtime.workspace_root().to_path_buf(), session_id.clone());
        let job = record.id.clone();
        // Checked, recorded, and added under the lock the stop drains the
        // threads under, so a thread opened while the daemon stops is closed
        // here, neither left running nor recorded.
        let refused = {
            let mut live = self.live();
            if self.stopping.is_cancelled() {
                Some((runtime, stopping()))
            } else if let Err(error) = self.jobs.register(record) {
                let error = RpcError::new(
                    ErrorCode::InternalError,
                    format!("could not record thread: {error}"),
                );
                Some((runtime, error))
            } else {
                live.insert(job.clone(), Arc::new(LiveThread::new(runtime, config)));
                None
            }
        };
        if let Some((runtime, error)) = refused {
            runtime.shutdown().await;
            return Err(error);
        }
        tracing::info!(%job, session = %session_id, "opened thread");
        Ok(ThreadCreateResponse {
            thread: (&job).into(),
            session_id: session_id.into_string(),
        })
    }

    /// Starts a turn of `text` on a thread and returns once the turn saved
    /// its prompt. The turn runs in a daemon task from then on.
    pub(super) async fn prompt(
        &self,
        params: ThreadPromptParams,
    ) -> Result<ThreadPromptResponse, RpcError> {
        self.refuse_when_stopping()?;
        let (job, thread) = self.live_thread(&params.thread)?;
        let cancel = {
            let mut turn_cancel = thread.turn_cancel();
            match self.jobs.start_turn(&job) {
                Ok(()) => {}
                Err(StartTurnError::Busy) => {
                    return Err(RpcError::new(
                        ErrorCode::ThreadBusy,
                        "thread already has a turn in progress",
                    ))
                }
                Err(StartTurnError::Unknown(_)) => return Err(not_found()),
            }
            *turn_cancel = CancellationToken::new();
            turn_cancel.clone()
        };
        // Spawned before anything awaits, so a request dropped from here on,
        // as when its client goes away, still leaves the turn to run and
        // record its end.
        let start = TurnTask {
            job,
            jobs: Arc::clone(&self.jobs),
            thread,
            stopping: self.stopping.clone(),
            cancel,
        }
        .spawn(params.text);
        match start.await {
            Ok(PromptStart::Saved) => Ok(ThreadPromptResponse {}),
            Ok(PromptStart::Cancelled) => Err(RpcError::new(
                ErrorCode::TurnCancelled,
                "the turn was cancelled before it started",
            )),
            Ok(PromptStart::Interrupted) => Err(stopping()),
            Ok(PromptStart::Failed(error)) => Err(RpcError::new(ErrorCode::TurnFailed, error)),
            // The task answers for every turn it runs, unless it panicked.
            Err(_) => Err(RpcError::new(
                ErrorCode::InternalError,
                "the turn stopped unexpectedly; see the daemon log",
            )),
        }
    }

    /// Cancels a thread's turn, if one is under way, even one that has not
    /// begun its prompt yet.
    pub(super) fn cancel(
        &self,
        params: ThreadCancelParams,
    ) -> Result<ThreadCancelResponse, RpcError> {
        let (_, thread) = self.live_thread(&params.thread)?;
        thread.turn_cancel().cancel();
        thread.handle.cancel();
        Ok(ThreadCancelResponse {})
    }

    /// Stages `text` for the running turn's next model request. The receipt
    /// is dropped: the turn reports what it applied with its events.
    pub(super) fn steer(&self, params: ThreadSteerParams) -> Result<ThreadSteerResponse, RpcError> {
        let (_, thread) = self.live_thread(&params.thread)?;
        match thread.handle.steer(UserInput::text(params.text)) {
            Ok(_receipt) => Ok(ThreadSteerResponse {}),
            Err(SteerError::NotRunning) => Err(RpcError::new(
                ErrorCode::ThreadNotRunning,
                "thread has no running turn to steer",
            )),
            Err(SteerError::Refused(error)) => Err(RpcError::new(
                ErrorCode::SteerRefused,
                format!("could not steer thread: {error}"),
            )),
        }
    }

    /// Stops every thread for the daemon's exit. Call once `stopping` is
    /// cancelled: no thread opens or turn starts after that. Cancels each
    /// turn, waits until [`DRAIN_TIMEOUT`] after `stop_requested` for the
    /// turns to save and the sessions to tear down, and records every job
    /// still under way as interrupted.
    pub(super) async fn stop(&self, stop_requested: tokio::time::Instant) {
        let threads: Vec<_> = self.live().drain().map(|(_, thread)| thread).collect();
        for thread in &threads {
            // The turn stops as one its client lost, so the session keeps
            // what a later daemon resumes (#1661).
            thread.handle.close(Closing::Disconnected);
        }
        let teardowns = threads.iter().map(|thread| async {
            // Free once the thread's turn saved and recorded its end.
            let runtime = thread.runtime.lock().await.take();
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
        self.jobs.interrupt_unfinished();
    }

    /// The live thread `id` names. A thread an earlier daemon ran is known
    /// but not loaded.
    fn live_thread(&self, id: &rho_protocol::JobId) -> Result<(JobId, Arc<LiveThread>), RpcError> {
        let job = JobId::parse(id.as_str()).map_err(|_| not_found())?;
        if let Some(thread) = self.live().get(&job) {
            return Ok((job, Arc::clone(thread)));
        }
        match self.jobs.get(&job) {
            Some(_) => Err(RpcError::new(
                ErrorCode::ThreadNotLoaded,
                "thread is not loaded in this daemon",
            )),
            None => Err(not_found()),
        }
    }

    fn refuse_when_stopping(&self) -> Result<(), RpcError> {
        if self.stopping.is_cancelled() {
            Err(stopping())
        } else {
            Ok(())
        }
    }

    fn live(&self) -> MutexGuard<'_, HashMap<JobId, Arc<LiveThread>>> {
        // The map is only inserted into and drained, so a panic cannot leave
        // it half-changed.
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn not_found() -> RpcError {
    RpcError::new(ErrorCode::ThreadNotFound, "no such thread")
}

fn stopping() -> RpcError {
    RpcError::new(ErrorCode::DaemonStopping, "the daemon is stopping")
}
