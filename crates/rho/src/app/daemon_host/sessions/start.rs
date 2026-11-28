//! Starting a session's turns: a prompt's, run in a [`TurnTask`], which
//! the session's job records as `running`, or, for a session another job
//! started, as `queued` for a slot in the daemon-wide budget (#1714).
//!
//! Every turn starts under a new [`TurnId`], which its prompt answers with
//! and its events carry. A prompt that names an idempotency key is safe to
//! send again (#1777): while the session knows the key (see
//! [`prompt_keys`](super::prompt_keys)), and the turn it started began or
//! may yet, the prompt starts nothing and answers as the first one does.

use rho_protocol::{
    ErrorCode, IdempotencyKey, MessageId, ProjectTrustMismatch, PromptAuthor, RpcError,
    SessionPromptAnswer, SessionPromptParams, SessionPromptResponse, SessionSetup, TurnId,
};
use rho_sdk::CancellationToken;
use tokio::sync::watch;

use super::{
    super::{
        jobs::{JobState, StartTurnError, TurnRecord},
        subagents::TurnBudget,
        wire::state_event,
    },
    being_deleted, ended_early, grant,
    prompt_keys::{row_key, KeyDigest, PromptMessage, SentBefore},
    stopping, Hold, LoadedSession, OpenWith, PromptStart, Queued, Sessions, TurnKind, TurnLimit,
    TurnSlot, TurnTask,
};
use crate::{
    app::{prompt_content::Prompt, session_runtime::SessionRuntime},
    session::tree::MessageKey,
    session_environment::TrustMismatch,
};

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
    /// The user message the turn inserts, once it has.
    pub(super) message: PromptMessage,
    /// The key the turn saves its user message under, when its prompt named
    /// an idempotency key (see [`row_key`]).
    pub(super) user_key: Option<MessageKey>,
    /// When the daemon cancels the turn as timed out; `None` sets no limit.
    /// Given to [`Sessions::start_turn`], so the turn runs under the limit
    /// its record was told of: a restart continues only a turn without one
    /// (see [`TurnRecord::continues_after_restart`]).
    pub(super) limit: Option<TurnLimit>,
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
    /// Its key is one an earlier prompt named, whose turn it starts no
    /// second time: it answers as that turn's prompt does.
    SentAgain(SentBefore),
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
    ) -> Result<SessionPromptAnswer, RpcError> {
        self.refuse_when_stopping()?;
        let key = params.idempotency_key;
        if key.as_ref().is_some_and(|key| key.as_str().is_empty()) {
            return Err(RpcError::new(
                ErrorCode::InvalidParams,
                "the prompt's idempotency key is empty",
            ));
        }
        if params.author == PromptAuthor::Unknown {
            return Err(RpcError::new(
                ErrorCode::InvalidParams,
                "the prompt's author is one this daemon does not know",
            ));
        }
        let prompt = Prompt::parse(&params.content).map_err(|error| {
            RpcError::new(
                ErrorCode::InvalidParams,
                format!("could not read the prompt: {error}"),
            )
        })?;
        // A prompt carries no setup, so a session it reopens opens with what
        // its file keeps alone.
        let with = OpenWith::under(
            grant::asked(params.grant, &params.environment)?,
            &params.environment,
            SessionSetup::default(),
        )?;
        // How the prompt is answered once the turn began: as the one that
        // started it, or as one sent again.
        type Answer = fn(SessionPromptResponse, Option<MessageId>) -> SessionPromptAnswer;
        let (
            SentBefore {
                id,
                mut start,
                message,
            },
            answer,
        ): (_, Answer) = loop {
            let (session, entered) = self
                .enter_session(&params.session, &with, |session, turn| {
                    let limit = params.timeout_ms.map(TurnLimit::starting_now);
                    let entered = self.start_turn(session, turn, &with, key.clone(), limit)?;
                    if matches!(entered, Entered::Started(_)) {
                        self.claim_starter(session, key.as_ref());
                    }
                    Ok(entered)
                })
                .await?;
            match entered {
                Entered::Started(turn) => {
                    let sent = SentBefore {
                        id: turn.id.clone(),
                        start: turn.start.subscribe(),
                        message: turn.message.clone(),
                    };
                    // Spawned before anything awaits, so the turn runs and
                    // records its end apart from this request: the
                    // transport joins every request it read, but this one
                    // stops waiting once the daemon stops.
                    TurnTask {
                        session,
                        turn,
                        stopping: self.stopping.clone(),
                    }
                    .spawn(prompt, params.author);
                    break (sent, SessionPromptAnswer::new);
                }
                Entered::SentAgain(sent) => break (sent, SessionPromptAnswer::retry),
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
                Ok(PromptStart::Began) => {
                    Ok(answer(SessionPromptResponse { turn: id }, message.get()))
                }
                Ok(PromptStart::Ended(outcome)) => Err(ended_early(outcome)),
                // Only a turn the daemon's stop interrupted, or one whose
                // task the exiting runtime dropped, never says.
                Ok(PromptStart::Pending) | Err(_) => Err(stopping()),
            },
            () = self.stopping.cancelled() => Err(stopping()),
        }
    }

    /// Starts a turn of a prompt sent with `with`, naming `key`, under
    /// `limit`, on `session`, whose turn lock `turn` is held, unless the
    /// session is busy or being deleted, or the prompt was sent before.
    pub(super) fn start_turn(
        &self,
        session: &LoadedSession,
        turn: &mut TurnSlot,
        with: &OpenWith,
        key: Option<IdempotencyKey>,
        limit: Option<TurnLimit>,
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
        let digest = key.as_ref().map(|key| KeyDigest::of(&session.id, key));
        if let Some(sent) = digest
            .as_ref()
            .and_then(|digest| turn.keys.sent_before(digest))
        {
            return Ok(Entered::SentAgain(sent));
        }
        // A restart continues the turn with one the daemon starts, under no
        // limit (see `restart_resume`): only for a client that takes such
        // turns, and only for a turn that had no limit to lose.
        let record = TurnRecord {
            continues_after_restart: session.idle_wake.is_some() && limit.is_none(),
            ..TurnRecord::starting(digest.clone())
        };
        let id = record.id.clone();
        let (budget, grant) = match self
            .jobs
            .start_turn(&session.job, record, with.grant.clone())
        {
            // Told under the turn lock, which the last turn's end is told
            // under too, so the feed never says `running` before the last
            // turn's end, nor a turn's end after the next turn began.
            Ok((moved, grant)) => {
                // This turn takes the place of the one that would continue
                // a turn the restart interrupted (see `restart_resume`).
                self.restart_resumes.take(&session.id);
                let budget = (moved.state == JobState::Queued)
                    .then(|| self.subagents.queue_turn(&session.job));
                // A turn takes the session's results, so whatever gave up
                // waking it for them looks again once this turn ends.
                session.rearm_wake();
                // Only the prompt that started it says whether it owes a
                // session a notice (see `peer_wake`).
                session.peer_wake.forget();
                // A turn the daemon starts to deliver finished work has yet
                // to take it, so clients hear how that work ended before
                // they hear the turn begin.
                session.report_background();
                session.feed.publish([state_event(&moved)]);
                (budget, grant)
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
        // project trust is the session's, carrying the grant the job gave
        // the turn: less than the prompt asked for when the job holds a
        // limit.
        session
            .environment
            .replace(grant::carrying(&grant, &with.environment));
        turn.cancel = CancellationToken::new();
        turn.kind = TurnKind::Prompt;
        turn.prompts += 1;
        let (start, started) = watch::channel(PromptStart::Pending);
        let message = PromptMessage::default();
        let user_key = digest.as_ref().map(|digest| row_key(digest, &id));
        if let Some(digest) = digest {
            let sent = SentBefore {
                id: id.clone(),
                start: started,
                message: message.clone(),
            };
            turn.keys.turn_started(digest, sent);
        }
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
            message,
            user_key,
            limit,
        }))
    }
}

#[cfg(test)]
#[path = "start_tests.rs"]
mod tests;
