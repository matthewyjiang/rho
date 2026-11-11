//! The event pump for one turn: SDK run events, approval requests, plan
//! approvals, questions for the user, the session's subagents' questions
//! among them, and applied background results,
//! reported to the host in arrival order as [`SessionEvent`]s, with the
//! host's [`TurnControl`] answers routed back to whatever asked.
//!
//! The pump never waits on the host. Requests stay pending until the host
//! answers or the turn withdraws them: a cancel answers open approvals as
//! cancelled and withdraws plan approvals and questions, and the end of the
//! run withdraws its questions, which wait on the user rather than the run.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
};

use futures_util::stream::{FuturesUnordered, StreamExt};
use rho_sdk::{
    hooks::HookDispatcher, ApprovalRequestReceiver, CancellationToken, HostInputId,
    HostInputRefusal, HostInputResponse, PendingApproval, RunEvent, SessionId, UserInput,
};
use tokio::sync::{mpsc, oneshot};

use super::{
    delegation::OfferedDisplays,
    events::{
        cancelled_approval, ApprovalId, PlanApprovalId, PlanApprovalRequest, SessionEvent,
        TurnControl,
    },
    persist::TurnCheckpoint,
    turn::TurnIo,
    QuestionSupport,
};
use crate::{
    app::subagent_host_input::SubagentHostInputRequest,
    display_transcript::DisplayTranscript,
    tools::plan_exit::{PlanPermissionAnswer, PlanPermissionReceiver, PlanPermissionRequest},
};

pub(super) enum EventSource<'a> {
    Run(&'a mut rho_sdk::Run),
    #[cfg(test)]
    Channel(&'a mut tokio::sync::mpsc::UnboundedReceiver<RunEvent>),
}

impl EventSource<'_> {
    async fn next(&mut self) -> Option<RunEvent> {
        match self {
            Self::Run(run) => run.next_event().await,
            #[cfg(test)]
            Self::Channel(events) => events.recv().await,
        }
    }

    fn cancel_run(&mut self) {
        match self {
            Self::Run(run) => run.cancel(),
            #[cfg(test)]
            Self::Channel(_) => {}
        }
    }

    /// Answers a host input request. The run acknowledges asynchronously;
    /// nothing waits for it, because a request the run no longer holds
    /// (its tool finished or was cancelled) has nobody left to tell.
    fn answer(&mut self, id: HostInputId, answer: HostInputAnswer) {
        match self {
            Self::Run(run) => {
                let queued = match answer {
                    Ok(response) => run.request_respond(id, response).map(drop),
                    Err(refusal) => run.request_refuse(id, refusal).map(drop),
                };
                if let Err(error) = queued {
                    tracing::warn!(%error, "could not answer a host input request");
                }
            }
            #[cfg(test)]
            Self::Channel(_) => {}
        }
    }
}

type HostInputAnswer = Result<HostInputResponse, HostInputRefusal>;

pub(super) enum ApprovalSource<'a> {
    Receiver(Option<&'a mut ApprovalRequestReceiver>),
    #[cfg(test)]
    Channel(&'a mut tokio::sync::mpsc::UnboundedReceiver<PendingApproval>),
}

impl ApprovalSource<'_> {
    async fn next(&mut self) -> Option<PendingApproval> {
        match self {
            Self::Receiver(None) => std::future::pending().await,
            Self::Receiver(Some(receiver)) => receiver.recv().await,
            #[cfg(test)]
            Self::Channel(approvals) => approvals.recv().await,
        }
    }
}

pub(super) struct PumpSources<'a> {
    pub(super) cancel: CancellationToken,
    /// Whether the session's host can put the run's questions to a person.
    pub(super) questions: QuestionSupport,
    pub(super) events: EventSource<'a>,
    pub(super) approvals: ApprovalSource<'a>,
    /// Plan approvals from `exit_plan_mode`; `None` when the session has no
    /// plan handoff.
    pub(super) plan_approvals: Option<&'a mut PlanPermissionReceiver>,
    /// Background results offered to the run; tests of the event and
    /// approval plumbing pass `None`.
    pub(super) displays: Option<AppliedDisplays<'a>>,
    /// The session's hooks, told whenever the turn waits for a person, so a
    /// user away from every client can learn of it (#1660). They are told
    /// after the host has the request, so a hook that reacts by attaching a
    /// client finds it open.
    pub(super) hooks: HookDispatcher,
    pub(super) session: &'a SessionId,
    /// Questions the session's subagents ask, which wait for the same
    /// person; `None` when the session runs no subagents of its own.
    pub(super) child_questions: Option<&'a mut mpsc::Receiver<SubagentHostInputRequest>>,
}

/// Background results handed to the model during one turn. Each is shown
/// where the run applied it and saved with the turn so a reload replays it.
pub(super) struct AppliedDisplays<'a> {
    pub(super) offered: OfferedDisplays,
    pub(super) checkpoint: &'a TurnCheckpoint,
}

impl AppliedDisplays<'_> {
    /// Records the rows for an applied boundary input and returns them to
    /// show.
    fn record(&mut self, input: &UserInput) -> Option<DisplayTranscript> {
        let transcript = self.offered.applied(input)?;
        self.checkpoint
            .record_display(input, transcript.display_message());
        Some(transcript)
    }
}

/// A plan approval the host may still answer. It ends with the answer
/// passed on, or withdrawn when the tool stops waiting first.
type PlanWait = Pin<Box<dyn Future<Output = PlanSettled> + Send>>;

enum PlanSettled {
    Answered,
    Withdrawn(PlanApprovalId),
}

/// Requests the host has not answered yet.
#[derive(Default)]
struct Pending {
    approvals: BTreeMap<ApprovalId, PendingApproval>,
    plans: BTreeMap<PlanApprovalId, oneshot::Sender<PlanPermissionAnswer>>,
    questions: BTreeSet<HostInputId>,
    /// Where each open subagent question's answer goes. Dropping one tells
    /// the subagent nobody will answer.
    child_questions: BTreeMap<HostInputId, ChildReply>,
    next_approval: u64,
    next_plan: u64,
}

/// Takes a subagent's answer to its question.
type ChildReply = oneshot::Sender<Result<HostInputResponse, rho_sdk::Error>>;

/// Runs the turn's sources until the run's events end and no approval waits
/// on the host. Fails only when the host reports it could not deliver
/// ([`TurnControl::HostFailed`]) or stops answering altogether.
pub(super) async fn pump(sources: PumpSources<'_>, io: &mut TurnIo) -> anyhow::Result<()> {
    let PumpSources {
        cancel,
        questions,
        mut events,
        mut approvals,
        mut plan_approvals,
        mut displays,
        hooks,
        session,
        mut child_questions,
    } = sources;
    let mut cancelled = cancel.is_cancelled();
    let mut events_open = true;
    let mut approvals_open = true;
    let mut pending = Pending::default();
    let mut plan_waits: FuturesUnordered<PlanWait> = FuturesUnordered::new();
    loop {
        if !events_open && pending.approvals.is_empty() && plan_waits.is_empty() {
            return Ok(());
        }
        tokio::select! {
            biased;

            () = cancel.cancelled(), if !cancelled => {
                cancelled = true;
                events.cancel_run();
                for (id, mut approval) in std::mem::take(&mut pending.approvals) {
                    let _ = approval.respond(cancelled_approval());
                    io.emit(SessionEvent::ApprovalWithdrawn { id });
                }
                // Dropping a plan's reply reports its call as cancelled.
                plan_waits.clear();
                for id in std::mem::take(&mut pending.plans).into_keys() {
                    io.emit(SessionEvent::PlanApprovalWithdrawn { id });
                }
                // The run's cancellation ends the questions themselves.
                withdraw_questions(&mut pending, io);
            }

            event = events.next(), if events_open => {
                let Some(event) = event else {
                    events_open = false;
                    withdraw_questions(&mut pending, io);
                    io.emit(SessionEvent::RunClosed);
                    continue;
                };
                if let (RunEvent::BoundaryInputApplied { input, .. }, Some(displays)) =
                    (&event, displays.as_mut())
                {
                    if let Some(transcript) = displays.record(input) {
                        io.emit(SessionEvent::BoundaryApplied { transcript });
                    }
                }
                let asked = match &event {
                    RunEvent::HostInputRequested { request } => Some((request.clone(), None)),
                    RunEvent::ToolHostInputRequested { call_id, request } => {
                        Some((request.clone(), Some(call_id.clone())))
                    }
                    _ => None,
                };
                io.emit(SessionEvent::Run(event));
                if let Some((request, call_id)) = asked {
                    match (questions, cancelled) {
                        (QuestionSupport::Available, false) => {
                            pending.questions.insert(request.id().clone());
                            io.emit(SessionEvent::QuestionAsked {
                                id: request.id().clone(),
                                call_id,
                                request: request.clone(),
                            });
                            hooks.question_requested(session, &request);
                        }
                        // The run's cancellation ends the request.
                        (QuestionSupport::Available, true) => {}
                        // Rho offers no asking tools to such hosts; anything
                        // that asks anyway must not wait forever.
                        (QuestionSupport::Unavailable, _) => {
                            events.answer(request.id().clone(), Err(HostInputRefusal::Declined));
                        }
                    }
                }
            }

            approval = approvals.next(), if events_open && approvals_open => {
                let Some(mut approval) = approval else {
                    approvals_open = false;
                    continue;
                };
                if cancelled {
                    let _ = approval.respond(cancelled_approval());
                    continue;
                }
                let id = ApprovalId(pending.next_approval);
                pending.next_approval += 1;
                io.emit(SessionEvent::ApprovalRequested {
                    id,
                    request: approval.request().clone(),
                });
                hooks.approval_requested(session, approval.request());
                pending.approvals.insert(id, approval);
            }

            asked = next_child_question(child_questions.as_deref_mut()), if events_open => {
                let Some(asked) = asked else {
                    child_questions = None;
                    continue;
                };
                // Dropping the reply tells the subagent nobody will answer:
                // the turn is ending.
                if cancelled {
                    continue;
                }
                let SubagentHostInputRequest {
                    run_id,
                    agent_id,
                    parent_session_id: _,
                    request,
                    response,
                } = asked;
                io.emit(SessionEvent::Notice {
                    text: format!("agent {run_id} ({agent_id}) asks: {}", request.title()),
                });
                io.emit(SessionEvent::QuestionAsked {
                    id: request.id().clone(),
                    call_id: None,
                    request: request.clone(),
                });
                hooks.question_requested(session, &request);
                pending.child_questions.insert(request.id().clone(), response);
            }

            // Its subagent stopped waiting, as one the parent stopped does.
            id = abandoned_child_question(&mut pending.child_questions) => {
                pending.child_questions.remove(&id);
                io.emit(SessionEvent::QuestionWithdrawn { id });
            }

            request = next_plan_approval(plan_approvals.as_deref_mut()), if events_open => {
                let Some(request) = request else {
                    plan_approvals = None;
                    continue;
                };
                if cancelled {
                    // Dropping the reply reports the call as cancelled.
                    continue;
                }
                let id = PlanApprovalId(pending.next_plan);
                pending.next_plan += 1;
                let (answer, answered) = oneshot::channel();
                io.emit(SessionEvent::PlanApprovalRequested {
                    id,
                    request: PlanApprovalRequest {
                        call_id: request.call_id.clone(),
                        plan: request.plan.clone(),
                        targets: request.targets.clone(),
                    },
                });
                hooks.plan_approval_requested(session, &request.plan);
                pending.plans.insert(id, answer);
                plan_waits.push(Box::pin(wait_for_plan(id, request, answered)));
            }

            control = io.control.recv() => match control {
                Some(TurnControl::RespondApproval { id, decision }) => {
                    if let Some(mut approval) = pending.approvals.remove(&id) {
                        let _ = approval.respond(decision);
                    }
                }
                Some(TurnControl::AnswerPlan { id, answer }) => {
                    if let Some(reply) = pending.plans.remove(&id) {
                        let _ = reply.send(answer);
                    }
                }
                Some(TurnControl::AnswerQuestion { id, answer }) => {
                    if pending.questions.remove(&id) {
                        events.answer(id, answer);
                    } else if let Some(reply) = pending.child_questions.remove(&id) {
                        let _ = reply.send(answer.map_err(|refusal| rho_sdk::Error::Interrupted {
                            message: match refusal {
                                HostInputRefusal::Declined => {
                                    "delegated questionnaire declined by user"
                                }
                                // `Dismissed`, or a refusal added later.
                                _ => "delegated questionnaire cancelled by user",
                            }
                            .into(),
                            source: None,
                        }));
                    }
                }
                Some(TurnControl::HostFailed { message }) => return Err(anyhow::Error::msg(message)),
                // Nothing could answer the requests still pending, or any
                // later one.
                None => anyhow::bail!("the host stopped answering the turn"),
            },

            Some(settled) = plan_waits.next(), if !plan_waits.is_empty() => match settled {
                PlanSettled::Answered => {}
                PlanSettled::Withdrawn(id) => {
                    if pending.plans.remove(&id).is_some() {
                        io.emit(SessionEvent::PlanApprovalWithdrawn { id });
                    }
                }
            },
        }
    }
}

/// Stops waiting on every open question, the run's and its subagents'.
fn withdraw_questions(pending: &mut Pending, io: &TurnIo) {
    let children = std::mem::take(&mut pending.child_questions).into_keys();
    for id in std::mem::take(&mut pending.questions)
        .into_iter()
        .chain(children)
    {
        io.emit(SessionEvent::QuestionWithdrawn { id });
    }
}

/// The pending child question whose subagent no longer waits for an
/// answer; never resolves while every subagent waits.
async fn abandoned_child_question(pending: &mut BTreeMap<HostInputId, ChildReply>) -> HostInputId {
    std::future::poll_fn(|context| {
        for (id, reply) in pending.iter_mut() {
            if reply.poll_closed(context).is_ready() {
                return std::task::Poll::Ready(id.clone());
            }
        }
        std::task::Poll::Pending
    })
    .await
}

async fn next_child_question(
    questions: Option<&mut mpsc::Receiver<SubagentHostInputRequest>>,
) -> Option<SubagentHostInputRequest> {
    match questions {
        Some(questions) => questions.recv().await,
        None => std::future::pending().await,
    }
}

async fn next_plan_approval(
    approvals: Option<&mut PlanPermissionReceiver>,
) -> Option<PlanPermissionRequest> {
    match approvals {
        Some(approvals) => approvals.recv().await,
        None => std::future::pending().await,
    }
}

/// Passes the host's answer to the waiting tool, unless the tool stops
/// waiting first.
async fn wait_for_plan(
    id: PlanApprovalId,
    mut request: PlanPermissionRequest,
    answered: oneshot::Receiver<PlanPermissionAnswer>,
) -> PlanSettled {
    tokio::select! {
        biased;
        () = request.reply.closed() => PlanSettled::Withdrawn(id),
        answer = answered => match answer {
            Ok(answer) => {
                let _ = request.reply.send(answer);
                PlanSettled::Answered
            }
            Err(_) => PlanSettled::Withdrawn(id),
        },
    }
}

#[cfg(test)]
#[path = "pump_tests.rs"]
mod tests;
