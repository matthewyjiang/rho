//! `read` and `wait`, which observe a session on a connection of their own:
//! the daemon keeps one attachment per session per connection, so a shared
//! connection would let two calls replace each other's. Observing never
//! loads a session, so neither call starts anything of it, such as its MCP
//! servers, nor brings it the caller's environment (#1672).

use std::time::Duration;

use rho_protocol::{
    methods::{SessionDetach, SessionObserve},
    Cursor, DaemonNotification, InputRequest, JobState, Notifications, Replay, Seq,
    SessionAttachResponse, SessionDetachParams, SessionEvent, SessionId, SessionObserveParams,
    StreamId, TurnOutcome,
};

use super::{
    params::{format_cursor, WaitResult},
    transcript::Transcript,
    ControlError,
};
use crate::{app::daemon_cli, daemon::Connection};

/// An observation of one session, on its own connection.
struct Attached {
    connection: Connection,
    session: SessionId,
    response: SessionAttachResponse,
}

impl Attached {
    async fn open(session: &SessionId, after: Option<Cursor>) -> Result<Self, ControlError> {
        let connection = daemon_cli::connect().await?;
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
pub(super) async fn transcript(session: &SessionId) -> Result<String, ControlError> {
    let mut attached = Attached::open(session, None).await?;
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

/// Follows the session from `after` until its turn stops running or
/// `timeout` passes, and reports where it is.
pub(super) async fn wait(
    session: SessionId,
    after: Option<Cursor>,
    timeout: Duration,
) -> Result<WaitResult, ControlError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut attached = Attached::open(&session, after.clone()).await?;
    let response = &attached.response;
    // A tail resumes the caller's cursor; a reset starts the stream over.
    let resumed = after
        .filter(|cursor| response.replay == Replay::Tail && cursor.stream == response.stream)
        .map_or(Seq(0), |cursor| cursor.seq);
    let mut watch = Watch {
        stream: response.stream.clone(),
        seq: resumed,
        state: response.state,
        outcome: None,
        outcome_checkpoint: None,
        inputs: response.inputs.clone(),
    };
    // The replay arrives at once and ends in the state the attach reported;
    // the deadline bounds only the wait for live events.
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
    watch.finish_replay(attached.response.replay, attached.response.state);
    while !detached && turn_runs(watch.state) {
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

/// Whether the session's turn still runs, so `wait` keeps waiting. A turn
/// that waits for input stops `wait`: only a person can move it on.
fn turn_runs(state: JobState) -> bool {
    matches!(state, JobState::Running | JobState::Queued)
}

/// What `wait` has seen of the session so far.
struct Watch {
    stream: StreamId,
    seq: Seq,
    state: JobState,
    /// How the last turn ended, until another starts.
    outcome: Option<TurnOutcome>,
    /// The checkpoint before an outcome we cannot return until State arrives.
    outcome_checkpoint: Option<Seq>,
    inputs: Vec<InputRequest>,
}

impl Watch {
    /// Reconciles replayed events with the attach's authoritative snapshot.
    fn finish_replay(&mut self, replay: Replay, state: JobState) {
        self.state = state;
        if replay == Replay::Reset && turn_runs(state) {
            // Only a reset can replay the saved previous turn's outcome.
            // A tail's pending outcome must retain its checkpoint until State.
            self.outcome = None;
            self.outcome_checkpoint = None;
        }
    }

    fn push(&mut self, seq: Seq, event: &SessionEvent) {
        let before = self.seq;
        self.seq = self.seq.max(seq);
        match event {
            SessionEvent::State { state } => {
                self.state = *state;
                if turn_runs(*state) {
                    self.outcome = None;
                    self.outcome_checkpoint = None;
                }
            }
            SessionEvent::TurnEnded { outcome } => {
                self.outcome = Some(outcome.clone());
                self.outcome_checkpoint = Some(before);
            }
            SessionEvent::InputRequested { request } => {
                if !self.inputs.iter().any(|open| open.id == request.id) {
                    self.inputs.push((**request).clone());
                }
            }
            SessionEvent::InputResolved { id, .. } => self.inputs.retain(|open| open.id != *id),
            _ => {}
        }
    }

    fn finish(self, session: SessionId) -> WaitResult {
        let done = !turn_runs(self.state);
        let ended = done && self.state != JobState::WaitingForInput;
        // A deadline or detach may split TurnEnded from its terminal State.
        // Replay the outcome next time rather than acknowledging and losing it.
        let checkpoint = if ended {
            self.seq
        } else {
            self.outcome_checkpoint.unwrap_or(self.seq)
        };
        WaitResult {
            session,
            state: self.state,
            done,
            outcome: self.outcome.filter(|_| ended),
            inputs: if self.state == JobState::WaitingForInput {
                self.inputs
            } else {
                Vec::new()
            },
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
