//! `read`, `wait`, and [`Follow`], which observe a session on a connection
//! of their own: the daemon keeps one attachment per session per
//! connection, so a shared connection would let two calls replace each
//! other's. Observing never loads a session, so none of them starts
//! anything of it, such as its MCP servers, nor brings it the caller's
//! environment (#1672).
//!
//! A [`Follow`] reports the session's news: each time it is done (see
//! [`Watch::done`]) with something it had not reported, such as the session
//! settling or a new request for a person, and why the daemon ended the
//! follow. `wait` is a follow until its first news or its deadline; `rho mcp
//! serve`'s webhook watches keep one for as long as they watch.

use std::time::Duration;

use rho_protocol::{
    methods::{SessionDetach, SessionObserve},
    Cursor, DaemonNotification, DetachReason, InputRequest, InputRequestId, JobState,
    Notifications, Replay, Seq, SessionActivity, SessionAttachResponse, SessionDetachParams,
    SessionEvent, SessionId, SessionObserveParams, StreamId, TurnId, TurnOutcome,
};

use serde::{Deserialize, Serialize};

use super::{
    params::{format_cursor, WaitResult},
    transcript::Transcript,
    ControlError,
};
use crate::{
    app::daemon_cli::{self, Reach},
    daemon::{ConnectAs, Connection},
};

/// Which daemon a follow connects to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Connect {
    /// The daemon as `reach` allows.
    Reach(Reach),
    /// The daemon that told its followers it is restarting in place, once
    /// it answers again. Never starts one.
    Restarted,
}

/// What the connection read next for the observed session.
enum Notice {
    Event(Seq, SessionEvent),
    /// The daemon dropped the attachment.
    Detached(DetachReason),
    /// The connection closed.
    Closed,
}

/// An observation of one session, on its own connection.
struct Attached {
    connection: Connection,
    session: SessionId,
    response: SessionAttachResponse,
}

impl Attached {
    async fn open(
        connect: Connect,
        session: &SessionId,
        after: Option<Cursor>,
    ) -> Result<Self, ControlError> {
        let connection = match connect {
            Connect::Reach(reach) => daemon_cli::connect_with(reach, ConnectAs::Agent).await?,
            Connect::Restarted => daemon_cli::connect_restarted(ConnectAs::Agent).await?,
        };
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

    /// What the connection reads next of the session.
    async fn next(&mut self) -> Notice {
        let notifications: &mut Notifications = &mut self.connection.notifications;
        loop {
            match notifications.recv().await {
                Some(DaemonNotification::SessionEvent(params))
                    if params.session == self.session =>
                {
                    return Notice::Event(params.seq, params.event)
                }
                Some(DaemonNotification::SessionDetached(params))
                    if params.session == self.session =>
                {
                    return Notice::Detached(params.reason)
                }
                Some(_) => {}
                None => return Notice::Closed,
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
    let mut attached = Attached::open(Connect::Reach(reach), session, None).await?;
    let mut transcript = Transcript::default();
    for _ in 0..attached.response.replay_events {
        match attached.next().await {
            Notice::Event(_, event) => transcript.push(&event),
            Notice::Detached(_) => break,
            Notice::Closed => return Err(rho_protocol::ClientError::Closed.into()),
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
    // The replay arrives at once; the deadline bounds only the wait for
    // live events. A timeout polls `next` before its deadline, so a session
    // done already is reported even with no time to wait.
    let mut follow = Follow::open(Connect::Reach(reach), session, after).await?;
    let result = match tokio::time::timeout_at(deadline, follow.next()).await {
        Err(_elapsed) => follow.now(),
        Ok(Followed::News(result)) => result,
        // Dropped, as a lagging client is: report what was seen, and the
        // next call resumes from the cursor.
        Ok(Followed::Ended(FollowEnd::Detached(_))) => return Ok(follow.now()),
        Ok(Followed::Ended(FollowEnd::Closed)) => {
            return Err(rho_protocol::ClientError::Closed.into())
        }
    };
    follow.detach().await;
    Ok(result)
}

/// A follow of one session: where it is, and its news as it comes.
pub(crate) struct Follow {
    attached: Attached,
    watch: Watch,
    /// Why the follow ended, once it did.
    ended: Option<FollowEnd>,
    /// Whether it read an event past its replay.
    progressed: bool,
}

/// What [`Follow::next`] read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Followed {
    /// The session is done with something not reported yet: where it is.
    News(WaitResult),
    /// Nothing more follows on this follow.
    Ended(FollowEnd),
}

/// Why a follow ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FollowEnd {
    /// The daemon dropped it.
    Detached(DetachReason),
    /// The connection to the daemon closed.
    Closed,
}

/// What makes the session's state news: the turn it is of, the requests
/// open for a person, and whether it settled or waits for one. A follow
/// reports each key once while the session stays done with it; `rho mcp
/// serve`'s webhook watches keep the last one they delivered, so they tell
/// the settle from the `waiting_for_input` news of the same turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NewsKey {
    pub(crate) state: JobState,
    #[serde(default)]
    pub(crate) turn: Option<TurnId>,
    #[serde(default)]
    pub(crate) inputs: Vec<InputRequestId>,
}

impl NewsKey {
    /// The key of `news`, as a follow reported it.
    pub(crate) fn of(news: &WaitResult) -> Self {
        Self {
            state: news.state,
            turn: news.turn.clone(),
            inputs: news.inputs.iter().map(|open| open.id).collect(),
        }
    }
}

impl Follow {
    /// Observes `session` from `after` and reads the replay, so
    /// [`Self::now`] says where the session is.
    pub(super) async fn open(
        connect: Connect,
        session: SessionId,
        after: Option<Cursor>,
    ) -> Result<Self, ControlError> {
        let mut attached = Attached::open(connect, &session, after.clone()).await?;
        let response = &attached.response;
        // A tail resumes the caller's cursor; a reset starts the stream over.
        let resumed = after
            .filter(|cursor| response.replay == Replay::Tail && cursor.stream == response.stream)
            .map_or(Seq(0), |cursor| cursor.seq);
        let mut watch = Watch {
            session,
            stream: response.stream.clone(),
            seq: resumed,
            state: response.state,
            turn: response.turn.clone(),
            ended: None,
            inputs: response.inputs.clone(),
            activity: None,
            reported: None,
        };
        // The replay ends in the state the attach reported, then the
        // session's activity.
        let mut ended = None;
        for _ in 0..attached.response.replay_events {
            match attached.next().await {
                Notice::Event(seq, event) => watch.push(seq, &event),
                Notice::Detached(reason) => {
                    ended = Some(FollowEnd::Detached(reason));
                    break;
                }
                Notice::Closed => {
                    ended = Some(FollowEnd::Closed);
                    break;
                }
            }
        }
        watch.finish_replay(attached.response.state, attached.response.turn.clone());
        Ok(Self {
            attached,
            watch,
            ended,
            progressed: false,
        })
    }

    /// Where the session is, as far as the follow has read.
    pub(crate) fn now(&self) -> WaitResult {
        self.watch.result()
    }

    /// The session's next news, the first being where it is when it is
    /// done already, or why the follow ended. Once ended, it stays ended.
    /// Cancel-safe: an event is applied only once it was read.
    pub(crate) async fn next(&mut self) -> Followed {
        loop {
            if let Some(news) = self.watch.news() {
                return Followed::News(news);
            }
            if let Some(end) = self.ended {
                return Followed::Ended(end);
            }
            match self.attached.next().await {
                Notice::Event(seq, event) => {
                    self.progressed = true;
                    self.watch.push(seq, &event);
                }
                Notice::Detached(reason) => self.ended = Some(FollowEnd::Detached(reason)),
                Notice::Closed => self.ended = Some(FollowEnd::Closed),
            }
        }
    }

    /// Whether the follow read an event past its replay: a follow the
    /// daemon ends without one saw nothing a reopen would not replay.
    pub(crate) fn progressed(&self) -> bool {
        self.progressed
    }

    /// Ends the follow, telling the daemon unless it ended it.
    pub(crate) async fn detach(self) {
        if self.ended.is_none() {
            self.attached.detach().await;
        }
    }
}

/// Whether the session's turn still runs.
fn turn_runs(state: JobState) -> bool {
    matches!(state, JobState::Running | JobState::Queued)
}

/// What a follow has seen of the session so far.
struct Watch {
    session: SessionId,
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
    /// The news last reported, while the session is still done with it.
    reported: Option<NewsKey>,
}

/// A turn's end, as its `turn_ended` told it.
#[derive(Clone)]
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

    /// Whether the session is done: it waits for a person, or no turn runs
    /// and the session settled, so none starts but a prompt, or a subagent
    /// asks a person something. A stream that does not report activity is
    /// done once its turn stops running. A session that waits for input
    /// moves on only once someone answers: a person, or for a question, the
    /// session that started it.
    fn done(&self) -> bool {
        match self.state {
            JobState::WaitingForInput => true,
            state if turn_runs(state) => false,
            _ => self.activity.is_none_or(SessionActivity::is_settled) || !self.inputs.is_empty(),
        }
    }

    /// Where the session is, when it is done with something not reported
    /// yet: the follow's first look at a session done already, the session
    /// settling, or a new request for a person. Once it is not done, what
    /// it was done with is news again.
    fn news(&mut self) -> Option<WaitResult> {
        if !self.done() {
            self.reported = None;
            return None;
        }
        let key = NewsKey {
            state: self.state,
            turn: self.turn.clone(),
            inputs: self.inputs.iter().map(|open| open.id).collect(),
        };
        if self.reported.as_ref() == Some(&key) {
            return None;
        }
        self.reported = Some(key);
        Some(self.result())
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

    /// Where the session is, as `wait` reports it.
    fn result(&self) -> WaitResult {
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
            .as_ref()
            .filter(|end| Some(&end.turn) == self.turn.as_ref());
        // Until the outcome is final (the session is done with the turn
        // over), resume before the turn's end, so the next wait reads it
        // again and returns it, or a later turn's, such as one the daemon
        // starts to deliver a result. A deadline or detach that splits a
        // turn's end from its terminal state is held back the same way.
        let checkpoint = match end {
            Some(end) if !(done && turn_over) => end.before,
            Some(_) | None => self.seq,
        };
        WaitResult {
            session: self.session.clone(),
            state: self.state,
            done,
            outcome: end.filter(|_| turn_over).map(|end| end.outcome.clone()),
            turn: self.turn.clone(),
            inputs: if done {
                self.inputs.clone()
            } else {
                Vec::new()
            },
            // Seq 0 is no position: a cursor at it always resets.
            cursor: (checkpoint > Seq(0)).then(|| {
                format_cursor(&Cursor {
                    stream: self.stream.clone(),
                    seq: checkpoint,
                })
            }),
        }
    }
}

#[cfg(test)]
#[path = "follow_tests.rs"]
mod tests;
