//! Answers the followed daemon session's input requests from the TUI
//! (#1667), with the prompts the in-process TUI shows for the same asks:
//! the approval prompt, the plan-approval questionnaire, and the
//! questionnaire ([`super::daemon_asks`]).
//!
//! The TUI shows one request at a time, oldest first; the rest wait. Each
//! prompt replies over a channel, read once the person's input is handled
//! ([`App::answer_daemon_input`]), and the reply goes to the daemon as
//! `rho/session/answer`. A request another client answered, the turn
//! withdrew, or the daemon answered at its timeout closes its prompt with a
//! notice ([`App::close_daemon_input`]).
//!
//! Keys reach the prompt while the TUI follows the session's turn
//! (`daemon_turn`): a session waits for input only while a turn runs, and
//! the job's `waiting_for_input` state keeps the TUI following it, whoever
//! started the turn.
//!
//! A question with a timeout counts down to the daemon's own answer. Once
//! the person touches its form, which pauses the countdown, the TUI holds
//! the request (`rho/session/hold`) so the daemon waits for them, as the
//! protocol asks of a client.

use std::{
    collections::{HashSet, VecDeque},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::FutureExt;
use rho_protocol::{InputAnswer, InputRequest, InputRequestId, InputTimeout};
use rho_sdk::{ApprovalDecision, HostInputRequest};
use tokio::sync::oneshot;

use super::{
    daemon_asks::{approval_answer, plan_answer, questions_answer, DaemonPrompt},
    App, ComposerMode, DefaultTerminal, Entry, QuestionAnswerRequest, QuestionnaireCancelReason,
    QuestionnaireReply, QuestionnaireResponseChannel,
};
use crate::permission::PermissionMode;

/// The daemon input request the TUI shows, and those waiting behind it.
#[derive(Default)]
pub(super) struct DaemonAnswers {
    shown: Option<Shown>,
    waiting: VecDeque<InputRequest>,
    /// Requests whose questions the person cancelled, which cancels the
    /// turn: their withdrawal goes unsaid.
    cancelled: HashSet<InputRequestId>,
}

/// The request whose prompt is open.
struct Shown {
    id: InputRequestId,
    reply: Reply,
    /// Whether the request still times out, so the person's first touch of
    /// its form holds it.
    times_out: bool,
}

/// Where the open prompt replies.
enum Reply {
    Approval(oneshot::Receiver<ApprovalDecision>),
    PlanApproval {
        reply: oneshot::Receiver<QuestionnaireReply>,
        modes: Vec<PermissionMode>,
    },
    Questions(oneshot::Receiver<QuestionnaireReply>),
}

/// What the person did at the open prompt.
enum Reaction {
    Answer(InputAnswer),
    /// Cancelled a plan approval, which cancels the turn, as in process.
    CancelTurn,
    /// Cancelled questions: dismisses them, then cancels the turn if one
    /// runs. They may be a subagent's, which outlive the turn, so the
    /// cancel alone would leave the subagent waiting.
    DismissQuestions,
    /// The prompt closed without a reply: another client may answer.
    Nothing,
}

impl App {
    /// Puts `request` to the person once no older request's prompt is open.
    /// Requests wait in id order, which is the order the daemon opened them
    /// in, so one shown again after a failed answer keeps its place.
    pub(super) async fn ask_daemon_input(&mut self, request: InputRequest) {
        let waiting = &mut self.daemon_answers.waiting;
        let place = waiting.partition_point(|open| open.id < request.id);
        waiting.insert(place, request);
        self.show_next_daemon_input().await;
    }

    /// Closes request `id`'s prompt, or drops it from those waiting, and
    /// says how it closed; then shows the next.
    pub(super) async fn close_daemon_input(&mut self, id: InputRequestId, notice: String) {
        if self
            .daemon_answers
            .shown
            .as_ref()
            .is_some_and(|shown| shown.id == id)
        {
            self.withdraw_daemon_prompt();
            if self.daemon_turn_runs() {
                self.set_status("running");
            }
        } else {
            self.daemon_answers
                .waiting
                .retain(|request| request.id != id);
        }
        if !self.daemon_answers.cancelled.remove(&id) {
            self.insert_entry(&Entry::Notice(notice));
        }
        self.show_next_daemon_input().await;
    }

    /// Request `id` no longer times out: its countdown stops.
    pub(super) fn daemon_input_held(&mut self, id: InputRequestId) {
        if let Some(request) = self
            .daemon_answers
            .waiting
            .iter_mut()
            .find(|request| request.id == id)
        {
            request.timeout = None;
        }
        let Some(shown) = self
            .daemon_answers
            .shown
            .as_mut()
            .filter(|shown| shown.id == id)
        else {
            return;
        };
        shown.times_out = false;
        if let ComposerMode::Questionnaire(questionnaire) = self.input_ui.composer_mut() {
            questionnaire.pause_timeout();
        }
    }

    /// Closes every daemon request's prompt unanswered, as once the TUI
    /// stops following the session's turn: those still open are left to
    /// other clients.
    pub(super) fn drop_daemon_inputs(&mut self) {
        self.withdraw_daemon_prompt();
        self.daemon_answers.waiting.clear();
        self.daemon_answers.cancelled.clear();
    }

    /// Sends the open prompt's reply to the daemon once the person gave it,
    /// or holds its request once they started on it, then shows the next
    /// waiting request; run after each terminal event the running turn
    /// handles. While `cancelling`, as when that event cancels the turn,
    /// nothing new shows: the turn's end closes what it still asks.
    pub(super) async fn answer_daemon_input(
        &mut self,
        terminal: &mut DefaultTerminal,
        cancelling: bool,
    ) -> anyhow::Result<()> {
        match self.take_daemon_reaction() {
            None => self.hold_touched_daemon_input(terminal).await?,
            Some((id, Reaction::Answer(answer))) => {
                if let Some(session) = self.info.session.daemon.as_mut() {
                    session.answer(id, answer).await;
                }
                self.show_taken_daemon_views(terminal).await?;
            }
            // The cancel withdraws the request.
            Some((id, Reaction::CancelTurn)) => {
                self.daemon_answers.cancelled.insert(id);
                self.cancel_daemon_turn().await;
                return Ok(());
            }
            Some((id, Reaction::DismissQuestions)) => {
                self.daemon_answers.cancelled.insert(id);
                if let Some(session) = self.info.session.daemon.as_mut() {
                    session.answer(id, InputAnswer::Dismiss).await;
                }
                if self.daemon_turn_runs() {
                    self.cancel_daemon_turn().await;
                    return Ok(());
                }
                self.show_taken_daemon_views(terminal).await?;
            }
            Some((_, Reaction::Nothing)) => {}
        }
        // Run on every event, so requests a failed cancel left waiting
        // show at the next key.
        if !cancelling {
            self.show_next_daemon_input().await;
        }
        Ok(())
    }

    /// [`Self::answer_daemon_input`] while no turn runs, when the open
    /// prompt is a subagent's question. Returns whether the prompt shown
    /// changed.
    pub(super) async fn answer_idle_daemon_input(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if self.info.session.daemon.is_none() {
            return Ok(false);
        }
        let shown = |app: &Self| app.daemon_answers.shown.as_ref().map(|shown| shown.id);
        let before = shown(self);
        self.answer_daemon_input(terminal, /*cancelling*/ false)
            .await?;
        Ok(shown(self) != before)
    }

    /// Holds the open request once the person touched its form, which
    /// pauses its countdown.
    async fn hold_touched_daemon_input(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let touched = !self.questionnaire_timeout_running();
        let Some(id) = self
            .daemon_answers
            .shown
            .as_ref()
            .filter(|shown| shown.times_out && touched)
            .map(|shown| shown.id)
        else {
            return Ok(());
        };
        let Some(session) = self.info.session.daemon.as_mut() else {
            return Ok(());
        };
        // A failed hold leaves the daemon's deadline running, so the next
        // touch tries again.
        if session.hold(id).await {
            if let Some(shown) = self.daemon_answers.shown.as_mut() {
                shown.times_out = false;
            }
        }
        self.show_taken_daemon_views(terminal).await
    }

    /// What the person did at the open prompt, once they did something;
    /// the prompt is then no longer shown.
    fn take_daemon_reaction(&mut self) -> Option<(InputRequestId, Reaction)> {
        let shown = self.daemon_answers.shown.as_mut()?;
        let reaction = match &mut shown.reply {
            Reply::Approval(reply) => match reply.now_or_never()? {
                Ok(decision) => Reaction::Answer(approval_answer(decision)),
                Err(_) => Reaction::Nothing,
            },
            Reply::PlanApproval { reply, modes } => {
                questionnaire_reaction(reply.now_or_never()?, |response| {
                    plan_answer(response, modes)
                })
            }
            Reply::Questions(reply) => {
                match questionnaire_reaction(reply.now_or_never()?, questions_answer) {
                    Reaction::CancelTurn => Reaction::DismissQuestions,
                    reaction @ (Reaction::Answer(_)
                    | Reaction::DismissQuestions
                    | Reaction::Nothing) => reaction,
                }
            }
        };
        let id = shown.id;
        self.daemon_answers.shown = None;
        Some((id, reaction))
    }

    /// Opens the oldest waiting request's prompt while none is open. One
    /// the TUI cannot show says so, and is left to other clients.
    async fn show_next_daemon_input(&mut self) {
        while self.daemon_answers.shown.is_none() {
            let Some(request) = self.daemon_answers.waiting.pop_front() else {
                return;
            };
            let shown = match DaemonPrompt::of(&request) {
                Ok(prompt) => self.open_daemon_prompt(&request, prompt).await,
                Err(error) => Err(error),
            };
            match shown {
                Ok(reply) => {
                    self.daemon_answers.shown = Some(Shown {
                        id: request.id,
                        reply,
                        times_out: request.timeout.is_some(),
                    });
                }
                Err(error) => self.insert_entry(&Entry::Error(format!(
                    "could not show the request: {error}"
                ))),
            }
        }
    }

    async fn open_daemon_prompt(
        &mut self,
        request: &InputRequest,
        prompt: DaemonPrompt,
    ) -> Result<Reply, String> {
        match prompt {
            DaemonPrompt::Approval(prompt) => {
                let (sender, reply) = oneshot::channel();
                self.open_replying_approval(prompt, sender).await;
                Ok(Reply::Approval(reply))
            }
            DaemonPrompt::PlanApproval { form, modes } => {
                let reply = self
                    .open_daemon_questionnaire(form, /*timeout*/ None)
                    .await?;
                Ok(Reply::PlanApproval { reply, modes })
            }
            DaemonPrompt::Questions(form) => self
                .open_daemon_questionnaire(form, request.timeout.as_ref())
                .await
                .map(Reply::Questions),
        }
    }

    /// Opens the questionnaire for `form`, counting down to `timeout`'s
    /// deadline, when the daemon answers it itself.
    async fn open_daemon_questionnaire(
        &mut self,
        form: HostInputRequest,
        timeout: Option<&InputTimeout>,
    ) -> Result<oneshot::Receiver<QuestionnaireReply>, String> {
        let (sender, reply) = oneshot::channel();
        self.open_questionnaire(QuestionAnswerRequest {
            request: form,
            response: QuestionnaireResponseChannel::new(sender),
            notice: None,
        })
        .await
        .map_err(|error| error.to_string())?;
        if let (Some(timeout), ComposerMode::Questionnaire(questionnaire)) =
            (timeout, self.input_ui.composer_mut())
        {
            // Replaces the timer of the form's own the user's config may
            // have started: only the daemon answers with the fallback.
            let remaining = Duration::from_millis(timeout.deadline_ms.saturating_sub(now_ms()));
            questionnaire.count_down_to_other_answer(remaining, Instant::now());
        }
        Ok(reply)
    }

    /// Closes the open prompt, if any, unanswered.
    fn withdraw_daemon_prompt(&mut self) {
        let Some(shown) = self.daemon_answers.shown.take() else {
            return;
        };
        match shown.reply {
            Reply::Approval(_) => self.withdraw_approval(),
            Reply::PlanApproval { .. } | Reply::Questions(_) => self.withdraw_questionnaire(),
        }
    }
}

/// What a questionnaire's `reply` comes to, its answers read by `answer`.
fn questionnaire_reaction(
    reply: Result<QuestionnaireReply, oneshot::error::RecvError>,
    answer: impl FnOnce(&rho_sdk::HostInputResponse) -> InputAnswer,
) -> Reaction {
    match reply {
        Ok(QuestionnaireReply::Answer(response)) => Reaction::Answer(answer(&response)),
        Ok(QuestionnaireReply::Cancelled(QuestionnaireCancelReason::UserCancelled)) => {
            Reaction::CancelTurn
        }
        Ok(QuestionnaireReply::Cancelled(QuestionnaireCancelReason::UiUnavailable)) | Err(_) => {
            Reaction::Nothing
        }
    }
}

/// Unix time now, in milliseconds, as input request deadlines count it.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
