//! Starting a session's turns: a prompt's, run in a [`TurnTask`], which
//! the session's job records as `running`, or, for a session another job
//! started, as `queued` for a slot in the daemon-wide budget (#1714).
//!
//! Every turn starts under a new [`TurnId`], which its prompt answers with
//! and its events carry. A prompt that names an idempotency key is safe to
//! send again (#1777): while the key is that of the session's latest turn,
//! and that turn began or may yet, the prompt starts nothing and answers as
//! the first one does.

use rho_protocol::{
    ErrorCode, IdempotencyKey, RpcError, SessionPromptParams, SessionPromptResponse, SessionSetup,
    TurnId,
};
use rho_sdk::CancellationToken;
use tokio::sync::watch;

use super::{
    super::{
        jobs::{JobState, StartTurnError, TurnRecord},
        subagents::TurnBudget,
        wire::state_event,
    },
    being_deleted, ended_early,
    open::session_environment,
    stopping, Hold, KeyedTurn, LoadedSession, OpenWith, PromptStart, Queued, Sessions, TurnKind,
    TurnLimit, TurnSlot, TurnTask,
};
use crate::app::{prompt_content, session_runtime::SessionRuntime};

/// A turn a prompt started, which its [`TurnTask`] runs.
pub(super) struct StartedTurn {
    pub(super) id: TurnId,
    pub(super) cancel: CancellationToken,
    /// The turn's place in the queue on the session's runtime.
    pub(super) runtime: Queued<Option<SessionRuntime>>,
    /// The turn's place in the daemon-wide budget's queue, for a session
    /// another job started (#1714); `None` runs once it has the runtime.
    pub(super) budget: Option<TurnBudget>,
    /// Told how the turn began, once it has (see [`PromptStart`]).
    pub(super) start: watch::Sender<PromptStart>,
}

/// How a prompt entered its session.
pub(super) enum Entered {
    /// Its turn started.
    Started(StartedTurn),
    /// Its key is that of the session's latest turn, `id`, which it starts
    /// no second time: it answers as that turn's prompt does, by `start`.
    SentAgain {
        id: TurnId,
        start: watch::Receiver<PromptStart>,
    },
    /// It cancelled the wake wait under way (see [`TurnKind::WakeWait`]),
    /// and starts once that wait ended.
    BehindWakeWait,
}

impl Sessions {
    /// Starts a turn of the prompt's content on a session and returns once
    /// the turn began. The turn runs in a daemon task from then on. A
    /// session busy with a T3 wake wait gives way (see
    /// [`TurnKind::WakeWait`]). A prompt sent again with its key starts
    /// nothing and returns once the turn it started the first time began.
    pub(in crate::app::daemon_host) async fn prompt(
        &self,
        params: SessionPromptParams,
    ) -> Result<SessionPromptResponse, RpcError> {
        self.refuse_when_stopping()?;
        let key = params.idempotency_key;
        if key.as_ref().is_some_and(|key| key.as_str().is_empty()) {
            return Err(RpcError::new(
                ErrorCode::InvalidParams,
                "the prompt's idempotency key is empty",
            ));
        }
        let input = prompt_content::user_input(&params.content).map_err(|error| {
            RpcError::new(
                ErrorCode::InvalidParams,
                format!("could not read the prompt: {error}"),
            )
        })?;
        // A prompt carries no setup, so a session it reopens opens with what
        // its file keeps alone.
        let with = OpenWith {
            environment: session_environment(&params.environment)?,
            setup: SessionSetup::default(),
        };
        let (id, mut start) = loop {
            let (session, entered) = self
                .enter_session(&params.session, &with, |session, turn| {
                    self.start_turn(session, turn, &with, key.clone())
                })
                .await?;
            match entered {
                Entered::Started(turn) => {
                    let id = turn.id.clone();
                    let start = turn.start.subscribe();
                    // Spawned before anything awaits, so the turn runs and
                    // records its end apart from this request: the
                    // transport joins every request it read, but this one
                    // stops waiting once the daemon stops.
                    TurnTask {
                        session,
                        turn,
                        stopping: self.stopping.clone(),
                        limit: params.timeout_ms.map(TurnLimit::starting_now),
                    }
                    .spawn(input);
                    break (id, start);
                }
                Entered::SentAgain { id, start } => break (id, start),
                // Free once the cancelled wait recorded its end.
                Entered::BehindWakeWait => drop(session.runtime.lock().await),
            }
        };
        // An answer that is already in wins over the stop, so a prompt the
        // turn saved is not reported refused. Once the daemon stops, the
        // turn goes on until the stop drains it.
        tokio::select! {
            biased;
            start = start.wait_for(|start| *start != PromptStart::Pending) => match start.as_deref() {
                Ok(PromptStart::Began) => Ok(SessionPromptResponse { turn: id }),
                Ok(PromptStart::Ended(outcome)) => Err(ended_early(outcome)),
                // Only a turn the daemon's stop interrupted, or one whose
                // task the exiting runtime dropped, never says.
                Ok(PromptStart::Pending) | Err(_) => Err(stopping()),
            },
            () = self.stopping.cancelled() => Err(stopping()),
        }
    }

    /// Starts a turn of a prompt sent with `with`, naming `key`, on
    /// `session`, whose turn lock `turn` is held, unless the session is
    /// busy or being deleted, or the prompt was sent before.
    pub(super) fn start_turn(
        &self,
        session: &LoadedSession,
        turn: &mut TurnSlot,
        with: &OpenWith,
        key: Option<IdempotencyKey>,
    ) -> Result<Entered, RpcError> {
        match turn.hold {
            // The turn queues on the runtime behind the changes, so they
            // apply first, the prompt waiting for them.
            Some(Hold::Configuring(_)) | None => {}
            Some(Hold::Deleting) => return Err(being_deleted()),
        }
        if let Some(sent) = turn
            .keyed
            .as_ref()
            .filter(|sent| Some(&sent.key) == key.as_ref())
        {
            // A turn that ended before it began ran nothing: its key starts
            // a turn again.
            if !matches!(*sent.start.borrow(), PromptStart::Ended(_)) {
                return Ok(Entered::SentAgain {
                    id: sent.id.clone(),
                    start: sent.start.clone(),
                });
            }
        }
        let record = TurnRecord::starting(key.clone());
        let id = record.id.clone();
        let budget = match self.jobs.start_turn(&session.job, record) {
            // Told under the turn lock, which the last turn's end is told
            // under too, so the feed never says `running` before the last
            // turn's end, nor a turn's end after the next turn began.
            Ok(moved) => {
                let budget = (moved.state == JobState::Queued)
                    .then(|| self.subagents.queue_turn(&session.job));
                session.feed.publish([state_event(&moved)]);
                budget
            }
            Err(StartTurnError::Busy) => match turn.kind {
                TurnKind::Prompt => {
                    return Err(RpcError::new(
                        ErrorCode::SessionBusy,
                        "session already has a turn in progress",
                    ))
                }
                TurnKind::WakeWait => {
                    session.cancel_turn_under(turn);
                    return Ok(Entered::BehindWakeWait);
                }
            },
            // An upgrade found the daemon idle a moment before.
            Err(StartTurnError::Closed) => return Err(stopping()),
            Err(StartTurnError::Unknown(_)) => {
                return Err(RpcError::new(
                    ErrorCode::InternalError,
                    format!("session's job {} is not recorded", session.job),
                ))
            }
        };
        // The turn's processes and hooks, and later turns' until another
        // prompt, start in the prompting client's environment.
        session.environment.replace(with.environment.clone());
        turn.cancel = CancellationToken::new();
        turn.kind = TurnKind::Prompt;
        let (start, started) = watch::channel(PromptStart::Pending);
        // The latest turn's key alone is kept: this turn's, or none.
        turn.keyed = key.map(|key| KeyedTurn {
            key,
            id: id.clone(),
            start: started,
        });
        // Queued on the runtime under the turn lock the turn starts under,
        // as a change queues under it (see `configure`): a change counted
        // before this applies before the turn, and one sent once the job
        // said `running` waits for its end.
        Ok(Entered::Started(StartedTurn {
            id,
            cancel: turn.cancel.clone(),
            runtime: Queued::join(&session.runtime),
            budget,
            start,
        }))
    }
}

#[cfg(test)]
#[path = "start_tests.rs"]
mod tests;
