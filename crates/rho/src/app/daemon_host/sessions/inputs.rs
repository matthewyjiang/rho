//! A session's open input requests (#1660): the approvals, plan approvals,
//! and questions its turn waits on a person for, in the daemon rather than
//! in any client.
//!
//! The turn opens and withdraws requests through its events; any client
//! answers one with `rho/session/answer`. The first answer takes the
//! request out, so a later one finds it gone. Nothing answers a request
//! because a client left: one with nobody attached stays open until someone
//! answers it or the turn withdraws it.

use std::collections::{btree_map::Entry, BTreeMap};

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
#[derive(Default)]
pub(super) struct Inputs {
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
}

/// One open request, and where its answer goes. The runtime's ids are
/// unique only within the model turn that asked, which `control` names.
struct Open {
    /// The asking model turn's answer channel.
    control: mpsc::UnboundedSender<TurnControl>,
    waiting: Waiting,
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
    /// Whether the turn waits on a person.
    pub(super) fn waiting(&self) -> bool {
        !self.open.is_empty()
    }

    /// What attached clients learn of the changes since the last call.
    pub(super) fn take_events(&mut self) -> Vec<wire::SessionEvent> {
        std::mem::take(&mut self.events)
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
    /// turn whose answers go to `control`.
    pub(super) fn note(
        &mut self,
        control: &mpsc::UnboundedSender<TurnControl>,
        event: &SessionEvent,
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
            | SessionEvent::RunClosed { .. }
            | SessionEvent::RunFinished { .. } => return,
        };
        let id = InputRequestId(self.next);
        self.next += 1;
        self.open.insert(
            id,
            Open {
                control: control.clone(),
                waiting,
            },
        );
        self.events.push(wire::SessionEvent::InputRequested {
            request: wire::InputRequest {
                id,
                ask,
                tool_call: tool_call.map(wire_tool_call),
            },
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

fn resolved(id: InputRequestId, resolution: Resolution) -> wire::SessionEvent {
    wire::SessionEvent::InputResolved { id, resolution }
}

fn wire_tool_call(id: &ToolCallId) -> wire::ToolCallId {
    wire::ToolCallId::new(id.as_str())
}

fn not_open(id: InputRequestId) -> RpcError {
    RpcError::new(
        ErrorCode::InputNotOpen,
        format!(
            "input request {id} is not open: another client answered it, or the turn withdrew it"
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
                InputAnswer::ApprovePlan { .. }
                | InputAnswer::KeepPlanning
                | InputAnswer::Answers { .. }
                | InputAnswer::Decline
                | InputAnswer::Dismiss => {
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
                    let mode = asks::mode_from_wire(mode)
                        .filter(|mode| modes.contains(mode))
                        .ok_or_else(|| invalid("the plan approval does not offer that mode"))?;
                    PlanPermissionAnswer::Approve(mode)
                }
                InputAnswer::KeepPlanning => PlanPermissionAnswer::KeepPlanning,
                InputAnswer::AllowOnce
                | InputAnswer::AllowForSession
                | InputAnswer::Deny { .. }
                | InputAnswer::Answers { .. }
                | InputAnswer::Decline
                | InputAnswer::Dismiss => {
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
                InputAnswer::AllowOnce
                | InputAnswer::AllowForSession
                | InputAnswer::Deny { .. }
                | InputAnswer::ApprovePlan { .. }
                | InputAnswer::KeepPlanning => {
                    return Err(invalid("a question takes answers, decline, or dismiss"))
                }
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
