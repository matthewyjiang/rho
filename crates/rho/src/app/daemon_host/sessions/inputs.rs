//! A session's open input requests (#1660): the approvals, plan approvals,
//! and questions its turn waits on a person for, in the daemon rather than
//! in any client.
//!
//! The turn opens and withdraws requests through its events; any client
//! answers one with `rho/session/answer`. The first answer takes the
//! request out, so a later one finds it gone. Nothing answers a request
//! because a client left: one with nobody attached stays open until someone
//! answers it or the turn withdraws it.

use std::collections::BTreeMap;

use rho_protocol::{self as wire, ErrorCode, InputAnswer, InputRequestId, Resolution, RpcError};
use rho_sdk::{
    ApprovalDecision, HostInputId, HostInputRefusal, HostInputRequest, HostInputResponse,
    HostInputSource,
};
use tokio::sync::mpsc;

use super::super::asks;
use crate::{
    app::session_runtime::{ApprovalId, PlanApprovalId, SessionEvent, TurnControl},
    permission::PermissionMode,
    tools::plan_exit::PlanPermissionAnswer,
};

/// What a [`InputAnswer::Deny`] without a reason tells the tool.
const DENIED_BY_USER: &str = "denied by user";

/// The requests a session's turn has open, by the id clients know them by.
#[derive(Default)]
pub(super) struct Inputs {
    /// The id of the session's next request: ids are never reused while
    /// the daemon has the session loaded.
    next: u64,
    open: BTreeMap<InputRequestId, Open>,
}

/// One open request, and where its answer goes. The runtime settles every
/// request before the model turn that asked ends, so its ids, unique within
/// a model turn, name one.
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

    /// Opens or withdraws the request `event` names, if any, for the model
    /// turn whose answers go to `control`. Returns what attached clients
    /// learn of it.
    pub(super) fn note(
        &mut self,
        control: &mpsc::UnboundedSender<TurnControl>,
        event: &SessionEvent,
    ) -> Option<wire::SessionEvent> {
        let (waiting, ask) = match event {
            SessionEvent::ApprovalRequested { id, request } => {
                (Waiting::Approval(*id), asks::approval(request))
            }
            SessionEvent::PlanApprovalRequested { id, request } => (
                Waiting::Plan {
                    id: *id,
                    modes: request.targets.clone(),
                },
                asks::plan_approval(request),
            ),
            SessionEvent::QuestionAsked { id, request, .. } => (
                Waiting::Question {
                    id: id.clone(),
                    request: request.clone(),
                },
                asks::question(request),
            ),
            SessionEvent::ApprovalWithdrawn { id } => {
                return self
                    .withdraw(|waiting| matches!(waiting, Waiting::Approval(open) if open == id));
            }
            SessionEvent::PlanApprovalWithdrawn { id } => {
                return self.withdraw(
                    |waiting| matches!(waiting, Waiting::Plan { id: open, .. } if open == id),
                );
            }
            SessionEvent::QuestionWithdrawn { id } => {
                return self.withdraw(
                    |waiting| matches!(waiting, Waiting::Question { id: open, .. } if open == id),
                );
            }
            SessionEvent::MessageInserted(_)
            | SessionEvent::RunStarted
            | SessionEvent::Run(_)
            | SessionEvent::BoundaryApplied { .. }
            | SessionEvent::Notice { .. }
            | SessionEvent::RunClosed { .. }
            | SessionEvent::RunFinished { .. } => return None,
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
        Some(wire::SessionEvent::InputRequested {
            request: wire::InputRequest { id, ask },
        })
    }

    /// Takes out the request `asked` matches, which the turn no longer
    /// waits for. An answered request is already out.
    fn withdraw(&mut self, asked: impl Fn(&Waiting) -> bool) -> Option<wire::SessionEvent> {
        let id = *self.open.iter().find(|(_, open)| asked(&open.waiting))?.0;
        self.open.remove(&id);
        Some(resolved(id, Resolution::Withdrawn))
    }

    /// Takes out every open request, for a turn that ended. Its runtime
    /// settles each before it ends, so this finds some only when the turn
    /// failed or its task stopped.
    pub(super) fn withdraw_all(&mut self) -> Vec<wire::SessionEvent> {
        std::mem::take(&mut self.open)
            .into_keys()
            .map(|id| resolved(id, Resolution::Withdrawn))
            .collect()
    }

    /// Hands `answer` to the turn that waits for request `id`, and takes
    /// the request out. An answer the request does not take leaves it open.
    pub(super) fn answer(
        &mut self,
        id: InputRequestId,
        answer: InputAnswer,
    ) -> Result<wire::SessionEvent, RpcError> {
        let open = self.open.get(&id).ok_or_else(|| not_open(id))?;
        let control = control_for(&open.waiting, answer)?;
        let open = self.open.remove(&id).expect("the request was open");
        // A closed channel is a model turn that ended without withdrawing
        // the request, so nobody waits for it any more.
        open.control.send(control).map_err(|_| not_open(id))?;
        Ok(resolved(id, Resolution::Answered))
    }
}

fn resolved(id: InputRequestId, resolution: Resolution) -> wire::SessionEvent {
    wire::SessionEvent::InputResolved { id, resolution }
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
    match (waiting, answer) {
        (Waiting::Approval(id), answer) => {
            let decision = match answer {
                InputAnswer::AllowOnce => ApprovalDecision::AllowOnce,
                InputAnswer::AllowForSession => ApprovalDecision::AllowForSession,
                InputAnswer::Deny { reason } => ApprovalDecision::Deny {
                    reason: reason.unwrap_or_else(|| DENIED_BY_USER.to_owned()),
                },
                _ => {
                    return Err(invalid(
                        "an approval takes allow_once, allow_for_session, or deny",
                    ))
                }
            };
            Ok(TurnControl::RespondApproval { id: *id, decision })
        }
        (Waiting::Plan { id, modes }, answer) => {
            let answer = match answer {
                InputAnswer::ApprovePlan { mode } => {
                    let mode = asks::mode_from_wire(mode)
                        .filter(|mode| modes.contains(mode))
                        .ok_or_else(|| invalid("the plan approval does not offer that mode"))?;
                    PlanPermissionAnswer::Approve(mode)
                }
                InputAnswer::KeepPlanning => PlanPermissionAnswer::KeepPlanning,
                _ => {
                    return Err(invalid(
                        "a plan approval takes approve_plan or keep_planning",
                    ))
                }
            };
            Ok(TurnControl::AnswerPlan { id: *id, answer })
        }
        (Waiting::Question { id, request }, answer) => {
            let answer = match answer {
                InputAnswer::Answers { answers } => Ok(answered(request, answers)?),
                InputAnswer::Decline => Err(HostInputRefusal::Declined),
                InputAnswer::Dismiss => Err(HostInputRefusal::Dismissed),
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
