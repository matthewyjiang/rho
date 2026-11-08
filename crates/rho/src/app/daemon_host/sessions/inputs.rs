//! A session's open input requests (#1660): the approvals, plan approvals,
//! and questions its turn waits on a person for, in the daemon rather than
//! in any client.
//!
//! The turn opens and withdraws requests through its events; any client
//! answers one with `rho/session/answer`. The first answer takes the
//! request out, so a later one finds it gone. Nothing answers a request
//! because a client left: one with nobody attached stays open until someone
//! answers it or the turn withdraws it.
//!
//! The one exception is the user's own: with `[questionnaire]
//! timeout_seconds` set, a question whose agent offered fallback answers
//! times out, and the daemon answers it with them, unless a client answers
//! it first or holds it, as a client does once its user starts on it.

use std::{
    collections::{btree_map::Entry, BTreeMap},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rho_protocol::{self as wire, ErrorCode, InputAnswer, InputRequestId, Resolution, RpcError};
use rho_sdk::{
    ApprovalDecision, HostInputId, HostInputRefusal, HostInputRequest, HostInputResponse,
    HostInputSource, ToolCallId,
};
use tokio::sync::mpsc;

use crate::{
    app::session_runtime::{ApprovalId, PlanApprovalId, SessionEvent, TurnControl},
    permission::{PermissionMode, DENIED_BY_USER},
    tools::plan_exit::PlanPermissionAnswer,
};

mod asks;

/// The requests a session's turn has open, by the id clients know them by.
pub(super) struct Inputs {
    /// How long a question with fallback answers waits before the daemon
    /// answers it with them: the session's `[questionnaire]
    /// timeout_seconds`, if set.
    question_timeout: Option<Duration>,
    /// The id of the session's next request: ids are never reused while
    /// the daemon has the session loaded.
    next: u64,
    open: BTreeMap<InputRequestId, Open>,
    /// What attached clients learn of the changes since the last
    /// [`Self::take_events`].
    events: Vec<wire::SessionEvent>,
    /// Answers accepted but not yet handed to their turns: they go after
    /// clients learn of them ([`Self::send_answers`]), so nothing the turn
    /// does with an answer reaches clients before the answer does.
    answers: Vec<(mpsc::UnboundedSender<TurnControl>, TurnControl)>,
    /// Timeouts opened since the last [`Self::take_timeouts`]: each request
    /// times out after its duration unless it closes or is held first.
    timeouts: Vec<(InputRequestId, Duration)>,
}

/// One open request, and where its answer goes. The runtime's ids are
/// unique only within the model turn that asked, which `control` names.
struct Open {
    /// The asking model turn's answer channel.
    control: mpsc::UnboundedSender<TurnControl>,
    waiting: Waiting,
    /// What the turn gets if the request times out; `None` when it has no
    /// timeout or a client held it.
    timeout: Option<TurnControl>,
}

/// A request as the runtime knows it, with what checking an answer needs.
enum Waiting {
    Approval(ApprovalId),
    Plan {
        id: PlanApprovalId,
        modes: Vec<PermissionMode>,
    },
    Question {
        id: HostInputId,
        request: HostInputRequest,
    },
}

impl Inputs {
    /// No requests open yet. Questions with fallback answers time out after
    /// `question_timeout`, if set.
    pub(super) fn new(question_timeout: Option<Duration>) -> Self {
        Self {
            question_timeout,
            next: 0,
            open: BTreeMap::new(),
            events: Vec::new(),
            answers: Vec::new(),
            timeouts: Vec::new(),
        }
    }

    /// Whether the turn waits on a person.
    pub(super) fn waiting(&self) -> bool {
        !self.open.is_empty()
    }

    /// What attached clients learn of the changes since the last call.
    pub(super) fn take_events(&mut self) -> Vec<wire::SessionEvent> {
        std::mem::take(&mut self.events)
    }

    /// The timeouts opened since the last call, for the caller to run:
    /// [`Self::time_out`] each request once its duration passes.
    pub(super) fn take_timeouts(&mut self) -> Vec<(InputRequestId, Duration)> {
        std::mem::take(&mut self.timeouts)
    }

    /// Hands the answers accepted since the last call to their turns.
    pub(super) fn send_answers(&mut self) {
        for (control, answer) in self.answers.drain(..) {
            // A turn that ended since takes no answer, as one that withdrew
            // the request at the same moment ignores it.
            let _ = control.send(answer);
        }
    }

    /// Opens or withdraws the request `event` names, if any, for the model
    /// turn whose answers go to `control`, at `now`.
    pub(super) fn note(
        &mut self,
        control: &mpsc::UnboundedSender<TurnControl>,
        event: &SessionEvent,
        now: SystemTime,
    ) {
        let (waiting, ask, tool_call) = match event {
            SessionEvent::ApprovalRequested { id, request } => (
                Waiting::Approval(*id),
                asks::approval(request),
                request.tool_call_id(),
            ),
            SessionEvent::PlanApprovalRequested { id, request } => (
                Waiting::Plan {
                    id: *id,
                    modes: request.targets.clone(),
                },
                asks::plan_approval(request),
                Some(&request.call_id),
            ),
            SessionEvent::QuestionAsked {
                id,
                call_id,
                request,
            } => (
                Waiting::Question {
                    id: id.clone(),
                    request: request.clone(),
                },
                asks::question(request),
                call_id.as_ref(),
            ),
            SessionEvent::ApprovalWithdrawn { id } => {
                return self.withdraw(
                    control,
                    |waiting| matches!(waiting, Waiting::Approval(open) if open == id),
                );
            }
            SessionEvent::PlanApprovalWithdrawn { id } => {
                return self.withdraw(
                    control,
                    |waiting| matches!(waiting, Waiting::Plan { id: open, .. } if open == id),
                );
            }
            SessionEvent::QuestionWithdrawn { id } => {
                return self.withdraw(
                    control,
                    |waiting| matches!(waiting, Waiting::Question { id: open, .. } if open == id),
                );
            }
            SessionEvent::MessageInserted(_)
            | SessionEvent::RunStarted
            | SessionEvent::Run(_)
            | SessionEvent::BoundaryApplied { .. }
            | SessionEvent::Notice { .. }
            | SessionEvent::RunClosed
            | SessionEvent::RunFinished { .. } => return,
        };
        let id = InputRequestId(self.next);
        self.next += 1;
        // Only a question whose agent offered fallback answers times out,
        // and only when the session's questions do.
        let (wire_timeout, timeout) = match (&waiting, self.question_timeout) {
            (Waiting::Question { id: asked, request }, Some(after)) => request
                .timeout_fallback()
                .zip(asks::timeout(request, deadline_ms(now, after)))
                .map(|(fallback, wire_timeout)| {
                    self.timeouts.push((id, after));
                    let answer = TurnControl::AnswerQuestion {
                        id: asked.clone(),
                        answer: Ok(fallback.clone()),
                    };
                    (wire_timeout, answer)
                }),
            (Waiting::Question { .. }, None) | (Waiting::Approval(_) | Waiting::Plan { .. }, _) => {
                None
            }
        }
        .unzip();
        self.open.insert(
            id,
            Open {
                control: control.clone(),
                waiting,
                timeout,
            },
        );
        self.events.push(wire::SessionEvent::InputRequested {
            request: Box::new(wire::InputRequest {
                id,
                ask,
                tool_call: tool_call.map(wire_tool_call),
                timeout: wire_timeout,
            }),
        });
    }

    /// Takes out the request of the model turn whose answers go to
    /// `control` that `asked` matches, which the turn no longer waits for.
    /// An answered request is already out.
    fn withdraw(
        &mut self,
        control: &mpsc::UnboundedSender<TurnControl>,
        asked: impl Fn(&Waiting) -> bool,
    ) {
        let found = self
            .open
            .iter()
            .find(|(_, open)| open.control.same_channel(control) && asked(&open.waiting))
            .map(|(&id, _)| id);
        if let Some(id) = found {
            self.open.remove(&id);
            self.events.push(resolved(id, Resolution::Withdrawn));
        }
    }

    /// Takes out every open request, for a turn that ended. Its runtime
    /// settles each before it ends, so this finds some only when the turn
    /// failed or its task stopped.
    pub(super) fn withdraw_all(&mut self) {
        for id in std::mem::take(&mut self.open).into_keys() {
            self.events.push(resolved(id, Resolution::Withdrawn));
        }
    }

    /// Answers request `id` with its timeout's answer, if it is still open
    /// with a timeout: a request answered, withdrawn, or held since is left
    /// as it is.
    pub(super) fn time_out(&mut self, id: InputRequestId) {
        let Entry::Occupied(mut open) = self.open.entry(id) else {
            return;
        };
        let Some(answer) = open.get_mut().timeout.take() else {
            return;
        };
        let Open { control, .. } = open.remove();
        if control.is_closed() {
            self.events.push(resolved(id, Resolution::Withdrawn));
            return;
        }
        self.events.push(resolved(id, Resolution::TimedOut));
        self.answers.push((control, answer));
    }

    /// Stops request `id`'s timeout for good, so it waits for an answer. A
    /// request without one, or already held, is left as it is.
    pub(super) fn hold(&mut self, id: InputRequestId) -> Result<(), RpcError> {
        let open = self.open.get_mut(&id).ok_or_else(|| not_open(id))?;
        if open.timeout.take().is_some() {
            self.events.push(wire::SessionEvent::InputHeld { id });
        }
        Ok(())
    }

    /// Takes request `id` out with `answer`, for [`Self::send_answers`]
    /// to hand to the turn that waits for it. An answer the request does
    /// not take leaves it open.
    pub(super) fn answer(
        &mut self,
        id: InputRequestId,
        answer: InputAnswer,
    ) -> Result<(), RpcError> {
        let Entry::Occupied(open) = self.open.entry(id) else {
            return Err(not_open(id));
        };
        let answer = control_for(&open.get().waiting, answer)?;
        let Open { control, .. } = open.remove();
        // The model turn that asked ended, so nobody waits for the request;
        // its withdrawal may still be on the way, and will find it gone.
        if control.is_closed() {
            self.events.push(resolved(id, Resolution::Withdrawn));
            return Err(not_open(id));
        }
        self.events.push(resolved(id, Resolution::Answered));
        self.answers.push((control, answer));
        Ok(())
    }
}

/// `after` past `now`, in Unix milliseconds; saturates, as no deadline
/// that far out is ever reached.
fn deadline_ms(now: SystemTime, after: Duration) -> u64 {
    now.checked_add(after)
        .and_then(|deadline| deadline.duration_since(UNIX_EPOCH).ok())
        .map_or(u64::MAX, |deadline| {
            u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX)
        })
}

fn resolved(id: InputRequestId, resolution: Resolution) -> wire::SessionEvent {
    wire::SessionEvent::InputResolved { id, resolution }
}

fn wire_tool_call(id: &ToolCallId) -> wire::ToolCallId {
    wire::ToolCallId::new(id.as_str())
}

pub(super) fn not_open(id: InputRequestId) -> RpcError {
    RpcError::new(
        ErrorCode::InputNotOpen,
        format!(
            "input request {id} is not open: another client answered it, it timed out, or the turn withdrew it"
        ),
    )
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}

/// What the turn gets for `answer` to the request `waiting`, or why the
/// request does not take it.
fn control_for(waiting: &Waiting, answer: InputAnswer) -> Result<TurnControl, RpcError> {
    match waiting {
        Waiting::Approval(id) => {
            let decision = match answer {
                InputAnswer::AllowOnce => ApprovalDecision::AllowOnce,
                InputAnswer::AllowForSession => ApprovalDecision::AllowForSession,
                InputAnswer::Deny { reason } => ApprovalDecision::Deny {
                    reason: reason.unwrap_or_else(|| DENIED_BY_USER.to_owned()),
                },
                // Another kind of request's answer, or a kind a newer protocol
                // adds.
                _ => {
                    return Err(invalid(
                        "an approval takes allow_once, allow_for_session, or deny",
                    ))
                }
            };
            Ok(TurnControl::RespondApproval { id: *id, decision })
        }
        Waiting::Plan { id, modes } => {
            let answer = match answer {
                InputAnswer::ApprovePlan { mode } => {
                    let mode = PermissionMode::try_from(mode)
                        .ok()
                        .filter(|mode| modes.contains(mode))
                        .ok_or_else(|| invalid("the plan approval does not offer that mode"))?;
                    PlanPermissionAnswer::Approve(mode)
                }
                InputAnswer::KeepPlanning => PlanPermissionAnswer::KeepPlanning,
                // Another kind of request's answer, or a kind a newer protocol
                // adds.
                _ => {
                    return Err(invalid(
                        "a plan approval takes approve_plan or keep_planning",
                    ))
                }
            };
            Ok(TurnControl::AnswerPlan { id: *id, answer })
        }
        Waiting::Question { id, request } => {
            let answer = match answer {
                InputAnswer::Answers { answers } => Ok(answered(request, answers)?),
                InputAnswer::Decline => Err(HostInputRefusal::Declined),
                InputAnswer::Dismiss => Err(HostInputRefusal::Dismissed),
                // Another kind of request's answer, or a kind a newer protocol
                // adds.
                _ => return Err(invalid("a question takes answers, decline, or dismiss")),
            };
            Ok(TurnControl::AnswerQuestion {
                id: id.clone(),
                answer,
            })
        }
    }
}

/// The user's `answers` to `request`, checked against its questions.
fn answered(
    request: &HostInputRequest,
    answers: BTreeMap<String, Vec<String>>,
) -> Result<HostInputResponse, RpcError> {
    let response = answers.into_iter().fold(
        HostInputResponse::new().with_source(HostInputSource::User),
        |response, (question, values)| response.answer(question, values),
    );
    request
        .validate(&response)
        .map_err(|error| invalid(format!("the answers do not fit the questions: {error}")))?;
    Ok(response)
}

#[cfg(test)]
#[path = "inputs_tests.rs"]
mod tests;
