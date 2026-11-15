//! `read` and `wait`, which observe a session on a connection of their own:
//! the daemon keeps one attachment per session per connection, so a shared
//! connection would let two calls replace each other's. Observing never
//! loads a session, so neither call starts anything of it, such as its MCP
//! servers, nor brings it the caller's environment (#1672).

use std::time::Duration;

use rho_protocol::{
    methods::{SessionDetach, SessionObserve},
    Cursor, DaemonNotification, InputRequest, JobState, Notifications, Replay, Seq,
    SessionActivity, SessionAttachResponse, SessionDetachParams, SessionEvent, SessionId,
    SessionObserveParams, StreamId, TurnId, TurnOutcome,
};

use super::{
    params::{format_cursor, WaitResult},
    transcript::Transcript,
    ControlError,
};
use crate::{
    app::daemon_cli::{self, Reach},
    daemon::{ConnectAs, Connection},
};

/// An observation of one session, on its own connection.
struct Attached {
    connection: Connection,
    session: SessionId,
    response: SessionAttachResponse,
}

impl Attached {
    async fn open(
        reach: Reach,
        session: &SessionId,
        after: Option<Cursor>,
    ) -> Result<Self, ControlError> {
        let connection = daemon_cli::connect_with(reach, ConnectAs::Agent).await?;
        let response = connection
            .client
            .request::<SessionObserve>(&SessionObserveParams {
                session: session.clone(),
                after,
            })
            .await?;
        Ok(Self {
            connection,
            session: session.clone(),
            response,
        })
    }

    /// The session's next event, with its seq, or `None` once the daemon
    /// dropped the attachment.
    async fn next(&mut self) -> Result<Option<(Seq, SessionEvent)>, ControlError> {
        let notifications: &mut Notifications = &mut self.connection.notifications;
        loop {
            match notifications.recv().await {
                Some(DaemonNotification::SessionEvent(params))
                    if params.session == self.session =>
                {
                    return Ok(Some((params.seq, params.event)))
                }
                Some(DaemonNotification::SessionDetached(params))
                    if params.session == self.session =>
                {
                    return Ok(None)
                }
                Some(_) => {}
                None => return Err(rho_protocol::ClientError::Closed.into()),
            }
        }
    }

    /// Detaches. The connection closes when this drops either way; asking
    /// first lets the daemon drop the subscription at once.
    async fn detach(self) {
        let _ = self
            .connection
            .client
            .request::<SessionDetach>(&SessionDetachParams {
                session: self.session,
            })
            .await;
    }
}

/// The session's transcript as text: its saved turns, then the turn in
/// progress.
pub(super) async fn transcript(reach: Reach, session: &SessionId) -> Result<String, ControlError> {
    let mut attached = Attached::open(reach, session, None).await?;
    let mut transcript = Transcript::default();
    for _ in 0..attached.response.replay_events {
        match attached.next().await? {
            Some((_, event)) => transcript.push(&event),
            None => break,
        }
    }
    attached.detach().await;
    Ok(transcript.text())
}

/// Follows the session from `after` until it is done (see [`Watch::done`])
/// or `timeout` passes, and reports where it is.
pub(super) async fn wait(
    reach: Reach,
    session: SessionId,
    after: Option<Cursor>,
    timeout: Duration,
) -> Result<WaitResult, ControlError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut attached = Attached::open(reach, &session, after.clone()).await?;
    let response = &attached.response;
    // A tail resumes the caller's cursor; a reset starts the stream over.
    let resumed = after
        .filter(|cursor| response.replay == Replay::Tail && cursor.stream == response.stream)
        .map_or(Seq(0), |cursor| cursor.seq);
    let mut watch = Watch {
        stream: response.stream.clone(),
        seq: resumed,
        state: response.state,
        turn: response.turn.clone(),
        ended: None,
        inputs: response.inputs.clone(),
        activity: None,
    };
    // The replay arrives at once and ends in the state the attach reported,
    // then the session's activity; the deadline bounds only the wait for
    // live events.
    let mut replay = attached.response.replay_events;
    let mut detached = false;
    while replay > 0 {
        replay -= 1;
        match attached.next().await? {
            Some((seq, event)) => watch.push(seq, &event),
            None => {
                detached = true;
                break;
            }
        }
    }
    watch.finish_replay(attached.response.state, attached.response.turn.clone());
    while !detached && !watch.done() {
        match tokio::time::timeout_at(deadline, attached.next()).await {
            Err(_elapsed) => break,
            Ok(Ok(Some((seq, event)))) => watch.push(seq, &event),
            // Dropped, as a lagging client is: report what was seen, and
            // the next call resumes from the cursor.
            Ok(Ok(None)) => detached = true,
            Ok(Err(error)) => return Err(error),
        }
    }
    if !detached {
        attached.detach().await;
    }
    Ok(watch.finish(session))
}

/// Whether the session's turn still runs.
fn turn_runs(state: JobState) -> bool {
    matches!(state, JobState::Running | JobState::Queued)
}

/// What `wait` has seen of the session so far.
struct Watch {
    stream: StreamId,
    seq: Seq,
    state: JobState,
    /// The turn `state` is of.
    turn: Option<TurnId>,
    /// The last turn end seen.
    ended: Option<Ended>,
    /// The input requests open, the turn's and its subagents'.
    inputs: Vec<InputRequest>,
    /// Whether the session is settled, as it last said; `None` from a
    /// stream that does not say, as a subagent run's.
    activity: Option<SessionActivity>,
}

/// A turn's end, as its `turn_ended` told it.
struct Ended {
    turn: TurnId,
    outcome: TurnOutcome,
    /// The last seq seen before it: where a wait that cannot return the
    /// outcome as final yet resumes, so the next one reads it again.
    before: Seq,
}

impl Watch {
    /// The replay is over: the attach's snapshot says where the job is.
    fn finish_replay(&mut self, state: JobState, turn: Option<TurnId>) {
        self.state = state;
        self.turn = turn;
    }

    /// Whether `wait` is done: the session waits for a person, or no turn
    /// runs and the session settled, so none starts but a prompt, or a
    /// subagent asks a person something. A stream that does not report
    /// activity is done once its turn stops running. Only a person moves a
    /// session that waits for input on.
    fn done(&self) -> bool {
        match self.state {
            JobState::WaitingForInput => true,
            state if turn_runs(state) => false,
            _ => self.activity.is_none_or(SessionActivity::is_settled) || !self.inputs.is_empty(),
        }
    }

    fn push(&mut self, seq: Seq, event: &SessionEvent) {
        let before = self.seq;
        self.seq = self.seq.max(seq);
        match event {
            SessionEvent::State { state, turn } => {
                self.state = *state;
                self.turn.clone_from(turn);
            }
            SessionEvent::TurnEnded { turn, outcome } => {
                self.ended = Some(Ended {
                    turn: turn.clone(),
                    outcome: outcome.clone(),
                    before,
                });
            }
            SessionEvent::InputRequested { request } => {
                if !self.inputs.iter().any(|open| open.id == request.id) {
                    self.inputs.push((**request).clone());
                }
            }
            SessionEvent::InputResolved { id, .. } => self.inputs.retain(|open| open.id != *id),
            SessionEvent::Activity { state } => self.activity = Some(*state),
            _ => {}
        }
    }

    fn finish(self, session: SessionId) -> WaitResult {
        let done = self.done();
        // The last turn is over, whether or not the session settled: its
        // outcome is reported even while background work keeps `done`
        // false, so a session that never settles, as one with a task left
        // running, still hands back its answer.
        let turn_over = !turn_runs(self.state) && self.state != JobState::WaitingForInput;
        // Only the end of the turn the job's state is of counts: a reset
        // replays the turn before's.
        let end = self
            .ended
            .filter(|end| Some(&end.turn) == self.turn.as_ref());
        // Until the outcome is final (the session is done with the turn
        // over), resume before the turn's end, so the next wait reads it
        // again and returns it, or a later turn's, such as one the daemon
        // starts to deliver a result. A deadline or detach that splits a
        // turn's end from its terminal state is held back the same way.
        let checkpoint = match &end {
            Some(end) if !(done && turn_over) => end.before,
            Some(_) | None => self.seq,
        };
        WaitResult {
            session,
            state: self.state,
            done,
            outcome: end.filter(|_| turn_over).map(|end| end.outcome),
            turn: self.turn,
            inputs: if done { self.inputs } else { Vec::new() },
            // Seq 0 is no position: a cursor at it always resets.
            cursor: (checkpoint > Seq(0)).then(|| {
                format_cursor(&Cursor {
                    stream: self.stream,
                    seq: checkpoint,
                })
            }),
        }
    }
}

#[cfg(test)]
#[path = "follow_tests.rs"]
mod tests;
