//! Takes the questions a session's subagents ask, whether or not a turn
//! runs, as the in-process TUI's inbox does.
//!
//! A subagent outlives the turn that started it and may ask long after, so
//! the session, not the turn, owns its subagents' question queue. One
//! watcher per loaded session, started as the session is added, opens each
//! question as an input request of the session (`inputs`, [`Asker::Subagent`])
//! and gives the subagent whatever answer a client sends. No turn starts
//! and the model is not called: the subagent goes on with the answer, and
//! its result reaches the model as any other does.
//!
//! The watcher holds the session weakly and ends once the session is
//! dropped, which tells every subagent still asking that nobody will
//! answer.

use std::{
    sync::{Arc, Mutex, PoisonError, Weak},
    time::SystemTime,
};

use rho_protocol as wire;
use rho_sdk::{hooks::HookDispatcher, CancellationToken, SessionId};
use tokio::sync::mpsc;

use super::{inputs::Asker, LoadedSession, Sessions};
use crate::app::{
    session_runtime::{SessionEvent, SessionRuntime, TurnControl},
    subagent_host_input::{OpenChildQuestions, SubagentHostInputRequest},
};

/// A session's side of its watcher.
pub(super) struct ChildQuestions {
    /// The queue the session's subagents ask on, until the watcher takes it.
    asked: Mutex<Option<mpsc::Receiver<SubagentHostInputRequest>>>,
    /// Tells the session's hooks a question waits (#1721).
    hooks: HookDispatcher,
    /// Cancelled when the session is dropped.
    dropped: CancellationToken,
}

impl ChildQuestions {
    /// Takes over the questions of the subagents of the session `runtime`
    /// runs, when a person can be asked and it starts subagents.
    pub(super) fn of(runtime: &mut SessionRuntime) -> Option<Self> {
        let asked = runtime.host_takes_child_questions()?;
        Some(Self {
            asked: Mutex::new(Some(asked)),
            hooks: runtime.hooks(),
            dropped: CancellationToken::new(),
        })
    }
}

impl Drop for ChildQuestions {
    fn drop(&mut self) {
        self.dropped.cancel();
    }
}

impl Sessions {
    /// Starts `session`'s watcher, if it takes its subagents' questions.
    /// Call once, as the session is added.
    pub(super) fn watch_child_questions(&self, session: &Arc<LoadedSession>) {
        let Some(questions) = &session.child_questions else {
            return;
        };
        let Some(asked) = questions
            .asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let watcher = take_questions(
            Arc::downgrade(session),
            session.id.clone(),
            asked,
            questions.hooks.clone(),
        );
        let dropped = questions.dropped.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = dropped.cancelled() => {}
                () = watcher => {}
            }
        });
    }
}

/// Opens each question arriving on `asked` as an input request of
/// `session`, and gives its subagent the answer, until the session is gone.
async fn take_questions(
    session: Weak<LoadedSession>,
    id: SessionId,
    mut asked: mpsc::Receiver<SubagentHostInputRequest>,
    hooks: HookDispatcher,
) {
    // Where the session's input requests send the answers to these
    // questions, as a turn's go to the turn.
    let (control, mut answers) = mpsc::unbounded_channel();
    let mut open = OpenChildQuestions::default();
    loop {
        tokio::select! {
            asked = asked.recv() => {
                let (Some(asked), Some(session)) = (asked, session.upgrade()) else {
                    return;
                };
                let (notice, request) = open.open(asked);
                session.feed.publish([wire::SessionEvent::Notice { text: notice }]);
                let event = SessionEvent::QuestionAsked {
                    id: request.id().clone(),
                    call_id: None,
                    request: request.clone(),
                };
                session.change_inputs(|inputs| {
                    inputs.note(Asker::Subagent, &control, &event, SystemTime::now());
                });
                hooks.question_requested(&id, &request);
            }
            answer = answers.recv() => {
                let Some(session) = session.upgrade() else {
                    return;
                };
                match answer {
                    Some(TurnControl::AnswerQuestion { id, answer }) => {
                        if let Some(notice) = open.answer(&id, answer) {
                            session.feed.publish([wire::SessionEvent::Notice { text: notice }]);
                        }
                    }
                    // These requests are questions, and `control` is held
                    // here, so the channel stays open.
                    Some(
                        TurnControl::RespondApproval { .. }
                        | TurnControl::AnswerPlan { .. }
                        | TurnControl::HostFailed { .. },
                    )
                    | None => {}
                }
            }
            // Its subagent stopped waiting, as one the parent stopped does.
            abandoned = open.abandoned() => {
                let Some(session) = session.upgrade() else {
                    return;
                };
                let event = SessionEvent::QuestionWithdrawn { id: abandoned };
                session.change_inputs(|inputs| {
                    inputs.note(Asker::Subagent, &control, &event, SystemTime::now());
                });
            }
        }
    }
}
