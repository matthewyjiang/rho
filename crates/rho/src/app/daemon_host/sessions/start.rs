//! Starting a session's turns: a prompt's, run in a [`TurnTask`], which
//! the session's job records as `running`, or, for a session another job
//! started, as `queued` for a slot in the daemon-wide budget (#1714).

use rho_protocol::{
    ErrorCode, ProjectTrustMismatch, RpcError, SessionPromptParams, SessionPromptResponse,
    SessionSetup,
};
use rho_sdk::CancellationToken;

use super::{
    super::{
        jobs::{JobState, StartTurnError},
        subagents::TurnBudget,
        wire::state_event,
    },
    being_deleted, ended_early,
    open::session_environment,
    stopping, Hold, LoadedSession, OpenWith, PromptStart, Queued, Sessions, TurnKind, TurnLimit,
    TurnSlot, TurnTask,
};
use crate::{
    app::{prompt_content, session_runtime::SessionRuntime},
    session_environment::TrustMismatch,
};

/// A turn a prompt started, which its [`TurnTask`] runs.
pub(super) struct StartedTurn {
    pub(super) cancel: CancellationToken,
    /// The turn's place in the queue on the session's runtime.
    pub(super) runtime: Queued<Option<SessionRuntime>>,
    /// The turn's place in the daemon-wide budget's queue, for a session
    /// another job started (#1714); `None` runs once it has the runtime.
    pub(super) budget: Option<TurnBudget>,
}

/// The error for a prompt whose client's project trust is not its
/// session's, naming the variables that differ and what the client can do.
fn trust_mismatch(mismatch: &TrustMismatch) -> RpcError {
    let TrustMismatch {
        session_only,
        other_only,
    } = mismatch;
    let mut differences = Vec::new();
    if !session_only.is_empty() {
        differences.push(format!(
            "the session trusts its project through {}, which this client's environment does not set",
            session_only.join(", ")
        ));
    }
    if !other_only.is_empty() {
        differences.push(format!(
            "this client's environment sets {}, which the session was opened without",
            other_only.join(", ")
        ));
    }
    let names = |variables: &[&str]| variables.iter().map(|&name| name.to_owned()).collect();
    let data = serde_json::to_value(ProjectTrustMismatch {
        session_only: names(session_only),
        client_only: names(other_only),
    })
    .expect("a project trust mismatch serializes");
    RpcError::new(
        ErrorCode::ProjectTrustMismatch,
        format!(
            "{}, so the prompt did not run: open a session of your own, or prompt with the session's RHO_TRUST_PROJECT_* variables",
            differences.join(", and ")
        ),
    )
    .with_data(data)
}

/// How a prompt entered its session.
pub(super) enum Entered {
    /// Its turn started.
    Started(StartedTurn),
    /// It cancelled the wake wait under way (see [`TurnKind::WakeWait`]),
    /// and starts once that wait ended.
    BehindWakeWait,
}

impl Sessions {
    /// Starts a turn of the prompt's content on a session and returns once
    /// the turn began. The turn runs in a daemon task from then on. A
    /// session busy with a T3 wake wait gives way (see
    /// [`TurnKind::WakeWait`]).
    pub(in crate::app::daemon_host) async fn prompt(
        &self,
        params: SessionPromptParams,
    ) -> Result<SessionPromptResponse, RpcError> {
        self.refuse_when_stopping()?;
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
        let (session, started) = loop {
            let (session, entered) = self
                .enter_session(&params.session, &with, |session, turn| {
                    self.start_turn(session, turn, &with)
                })
                .await?;
            match entered {
                Entered::Started(started) => break (session, started),
                // Free once the cancelled wait recorded its end.
                Entered::BehindWakeWait => drop(session.runtime.lock().await),
            }
        };
        // Spawned before anything awaits, so the turn runs and records its
        // end apart from this request: the transport joins every request it
        // read, but this one stops waiting once the daemon stops.
        let start = TurnTask {
            session,
            runtime: started.runtime,
            budget: started.budget,
            stopping: self.stopping.clone(),
            cancel: started.cancel,
            limit: params.timeout_ms.map(TurnLimit::starting_now),
        }
        .spawn(input);
        // An answer that is already in wins over the stop, so a prompt the
        // turn saved is not reported refused. Once the daemon stops, the
        // turn goes on until the stop drains it.
        tokio::select! {
            biased;
            start = start => match start {
                Ok(PromptStart::Began) => Ok(SessionPromptResponse {}),
                Ok(PromptStart::Ended(outcome)) => Err(ended_early(&outcome)),
                // Only a turn the daemon's stop interrupted, or one whose
                // task the exiting runtime dropped, sends no answer.
                Err(_) => Err(stopping()),
            },
            () = self.stopping.cancelled() => Err(stopping()),
        }
    }

    /// Starts a turn of a prompt sent with `with` on `session`, whose turn
    /// lock `turn` is held, unless the session is busy or being deleted.
    pub(super) fn start_turn(
        &self,
        session: &LoadedSession,
        turn: &mut TurnSlot,
        with: &OpenWith,
    ) -> Result<Entered, RpcError> {
        match turn.hold {
            // The turn queues on the runtime behind the changes, so they
            // apply first, the prompt waiting for them.
            Some(Hold::Configuring(_)) | None => {}
            Some(Hold::Deleting) => return Err(being_deleted()),
        }
        // Before anything of the session changes: its job, its feed, and
        // the clients attached to it stay as they were.
        if let Some(mismatch) = session.environment.trust_mismatch(&with.environment) {
            return Err(trust_mismatch(&mismatch));
        }
        let budget = match self.jobs.start_turn(&session.job) {
            // Told under the turn lock, which the last turn's end is told
            // under too, so the feed never says `running` before the last
            // turn's end, nor a turn's end after the next turn began.
            Ok(state) => {
                let budget =
                    (state == JobState::Queued).then(|| self.subagents.queue_turn(&session.job));
                session.feed.publish([state_event(&state)]);
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
        // prompt, start in the prompting client's environment, whose
        // project trust is the session's.
        session.environment.replace(with.environment.clone());
        turn.cancel = CancellationToken::new();
        turn.kind = TurnKind::Prompt;
        // Queued on the runtime under the turn lock the turn starts under,
        // as a change queues under it (see `configure`): a change counted
        // before this applies before the turn, and one sent once the job
        // said `running` waits for its end.
        Ok(Entered::Started(StartedTurn {
            cancel: turn.cancel.clone(),
            runtime: Queued::join(&session.runtime),
            budget,
        }))
    }
}
