//! Shows one turn's [`SessionEvent`]s to the ACP client and answers the
//! turn's requests with the client's replies.
//!
//! Runs beside [`SessionRuntime::run_turn`] for the whole turn. Run events
//! become session updates in order, with the usage and checklist updates
//! they imply. Approvals, plan approvals, and questions become client
//! requests, which wait concurrently so a pending one never holds back the
//! event stream; a withdrawal drops its request. An update the client did
//! not take fails the turn: the forwarder tells the runtime once and then
//! only closes the run's cards, best effort. A turn whose run already
//! ended settles anyway (see `TurnControl::HostFailed`), and the prompt
//! fails with the delivery error.
//!
//! [`SessionRuntime::run_turn`]: crate::app::session_runtime::SessionRuntime::run_turn

use std::{collections::HashMap, future::Future, pin::Pin, sync::atomic::AtomicU64};

use agent_client_protocol::schema::v1::{
    PlanEntry, RequestPermissionOutcome, SessionId, SessionNotification,
};
use futures_util::{
    future::{AbortHandle, Abortable},
    stream::{FuturesUnordered, StreamExt},
};
use rho_sdk::{HostInputId, HostInputRefusal};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::send_notification;
use crate::app::{
    acp::{
        elicitation,
        events::{EventMapper, RunEnding},
        host_features::{Forms, HostFeatures},
        permission,
        prompt_acceptance::PromptAcceptance,
        signals::SessionSignals,
        AcpClientPort,
    },
    session_runtime::{ApprovalId, PlanApprovalId, RunEnd, SessionEvent, TurnControl},
};

/// What a turn needs to reach the client.
pub(super) struct Forwarder<'a> {
    pub(super) session_id: &'a SessionId,
    pub(super) client: &'a dyn AcpClientPort,
    pub(super) features: HostFeatures,
    pub(super) mapper: EventMapper,
    pub(super) signals: SessionSignals,
    pub(super) placeholders: &'a AtomicU64,
    /// Reported when the turn inserts the prompt's user message.
    pub(super) acceptance: &'a mut PromptAcceptance,
}

/// How forwarding went.
pub(super) struct Forwarded {
    /// The checklist the client last saw, when known: the starting one if
    /// the run never started, the final one if it finished. A run the host
    /// failed leaves it unknown, which costs a resend next prompt.
    pub(super) sent_plan: Option<Vec<PlanEntry>>,
    /// Why an update did not reach the client, if one did not.
    pub(super) failure: Option<String>,
}

/// A request waiting on the client, keyed for its withdrawal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Asked {
    Approval(ApprovalId),
    Plan(PlanApprovalId),
    Question(HostInputId),
}

/// A client request that ends in the turn's answer.
type Request<'a> = Pin<Box<dyn Future<Output = (Asked, TurnControl)> + Send + 'a>>;

/// One turn's forwarding state.
struct Forwarding<'a> {
    target: Forwarder<'a>,
    control: UnboundedSender<TurnControl>,
    requests: FuturesUnordered<Abortable<Request<'a>>>,
    open: HashMap<Asked, AbortHandle>,
    started: bool,
    finished: bool,
    failure: Option<String>,
}

impl<'a> Forwarder<'a> {
    /// Forwards until the turn drops its event sender, which it does as it
    /// returns.
    pub(super) async fn forward(
        self,
        mut events: UnboundedReceiver<SessionEvent>,
        control: UnboundedSender<TurnControl>,
    ) -> Forwarded {
        let mut forwarding = Forwarding {
            target: self,
            control,
            requests: FuturesUnordered::new(),
            open: HashMap::new(),
            started: false,
            finished: false,
            failure: None,
        };
        loop {
            tokio::select! {
                biased;

                event = events.recv() => match event {
                    Some(event) => forwarding.show(event).await,
                    None => break,
                },

                Some(answered) = forwarding.requests.next(), if !forwarding.requests.is_empty() => {
                    // An aborted request was withdrawn; nothing waits on it.
                    if let Ok((asked, answer)) = answered {
                        forwarding.open.remove(&asked);
                        let _ = forwarding.control.send(answer);
                    }
                }
            }
        }
        let Forwarding {
            target,
            started,
            finished,
            failure,
            ..
        } = forwarding;
        Forwarded {
            sent_plan: (!started || finished).then(|| target.signals.into_sent_plan()),
            failure,
        }
    }
}

impl<'a> Forwarding<'a> {
    async fn show(&mut self, event: SessionEvent) {
        if let Some(failure) = self.failure.clone() {
            // The turn is failing, or its run ended before the turn saw the
            // failure: close the run's cards if the client still listens,
            // and leave everything else.
            if let SessionEvent::RunClosed { ending } = &event {
                let message = match ending {
                    RunEnd::Failed(message) => message,
                    RunEnd::Ended => &failure,
                };
                self.close_failed(message).await;
            }
            return;
        }
        let session_id = self.target.session_id;
        match event {
            SessionEvent::MessageInserted(inserted) => self.target.acceptance.accept(inserted),
            SessionEvent::RunStarted => {
                self.started = true;
                self.target
                    .signals
                    .send_plan_change(session_id, self.target.client)
                    .await;
            }
            SessionEvent::Run(event) => {
                let notifications = self.target.mapper.map_event(session_id, &event);
                if !self.deliver(notifications).await {
                    return;
                }
                if let Some(update) = self.target.signals.observe(&event).await {
                    self.deliver([SessionNotification::new(session_id.clone(), update)])
                        .await;
                }
            }
            SessionEvent::BoundaryApplied { transcript } => {
                self.deliver(EventMapper::display_transcript(session_id, &transcript))
                    .await;
            }
            SessionEvent::ApprovalRequested { id, request } => {
                let placeholder =
                    permission::next_placeholder_tool_call_id(self.target.placeholders);
                let prompt = permission::permission_request(session_id, &request, &placeholder);
                let client = self.target.client;
                self.ask(
                    Asked::Approval(id),
                    Box::pin(async move {
                        let outcome = match client.request_permission(prompt).await {
                            Ok(response) => response.outcome,
                            Err(_) => RequestPermissionOutcome::Cancelled,
                        };
                        (
                            Asked::Approval(id),
                            TurnControl::RespondApproval {
                                id,
                                decision: permission::decision_for(&outcome),
                            },
                        )
                    }),
                );
            }
            SessionEvent::ApprovalWithdrawn { id } => self.withdraw(&Asked::Approval(id)),
            SessionEvent::PlanApprovalRequested { id, request } => {
                let prompt = permission::plan_approval_request(session_id, &request);
                let client = self.target.client;
                self.ask(
                    Asked::Plan(id),
                    Box::pin(async move {
                        let outcome = match client.request_permission(prompt).await {
                            Ok(response) => response.outcome,
                            Err(error) => {
                                tracing::warn!(%error, "ACP host failed a plan approval prompt");
                                RequestPermissionOutcome::Cancelled
                            }
                        };
                        (
                            Asked::Plan(id),
                            TurnControl::AnswerPlan {
                                id,
                                answer: permission::plan_answer_for(&outcome),
                            },
                        )
                    }),
                );
            }
            SessionEvent::PlanApprovalWithdrawn { id } => self.withdraw(&Asked::Plan(id)),
            SessionEvent::QuestionAsked {
                id,
                call_id,
                request,
            } => match self.target.features.forms {
                Forms::Available(dialect) => {
                    let form = elicitation::elicitation_request(
                        &request,
                        session_id,
                        call_id.as_ref(),
                        dialect,
                    );
                    let client = self.target.client;
                    let asked = Asked::Question(id.clone());
                    self.ask(
                        asked.clone(),
                        Box::pin(async move {
                            let answer = match client.create_elicitation(form).await {
                                Ok(response) => {
                                    elicitation::host_answer(&request, response, dialect)
                                }
                                Err(error) => {
                                    tracing::warn!(%error, "ACP host failed an elicitation request");
                                    Err(HostInputRefusal::Dismissed)
                                }
                            };
                            (asked, TurnControl::AnswerQuestion { id, answer })
                        }),
                    );
                }
                // Sessions on such hosts declare no question support, so the
                // turn declines before asking; this only keeps that true.
                Forms::Unavailable => {
                    let _ = self.control.send(TurnControl::AnswerQuestion {
                        id,
                        answer: Err(HostInputRefusal::Declined),
                    });
                }
            },
            SessionEvent::QuestionWithdrawn { id } => self.withdraw(&Asked::Question(id)),
            SessionEvent::Notice { text } => {
                let notice = EventMapper::info_notice(session_id, self.target.features, &text);
                if let Err(error) = send_notification(self.target.client, notice).await {
                    tracing::debug!(%error, "could not send ACP load notice");
                }
            }
            SessionEvent::RunClosed { ending } => match ending {
                RunEnd::Ended => {
                    let notifications = self
                        .target
                        .mapper
                        .close_unfinished(session_id, RunEnding::WithoutResult);
                    self.deliver(notifications).await;
                }
                RunEnd::Failed(message) => self.close_failed(&message).await,
            },
            SessionEvent::RunFinished { context } => {
                self.finished = true;
                // The committed turn is the settled context; in-run updates
                // were estimates.
                self.target
                    .signals
                    .send_usage_for(context, session_id, self.target.client)
                    .await;
            }
        }
    }

    /// Sends `notifications` in order; on the first failure, fails the turn
    /// and returns false.
    async fn deliver(
        &mut self,
        notifications: impl IntoIterator<Item = SessionNotification>,
    ) -> bool {
        for notification in notifications {
            if let Err(error) = send_notification(self.target.client, notification).await {
                let message = error.to_string();
                let _ = self.control.send(TurnControl::HostFailed {
                    message: message.clone(),
                });
                self.failure = Some(message);
                // The turn answers nothing more once it fails.
                for (_, handle) in self.open.drain() {
                    handle.abort();
                }
                return false;
            }
        }
        true
    }

    /// Best effort: the client may already be gone, and the turn's own
    /// error is the one the prompt reports.
    async fn close_failed(&mut self, message: &str) {
        let notifications = self
            .target
            .mapper
            .close_unfinished(self.target.session_id, RunEnding::Failed(message));
        for notification in notifications {
            if send_notification(self.target.client, notification)
                .await
                .is_err()
            {
                break;
            }
        }
    }

    fn ask(&mut self, asked: Asked, request: Request<'a>) {
        let (handle, registration) = AbortHandle::new_pair();
        self.open.insert(asked, handle);
        self.requests.push(Abortable::new(request, registration));
    }

    fn withdraw(&mut self, asked: &Asked) {
        if let Some(handle) = self.open.remove(asked) {
            handle.abort();
        }
    }
}

#[cfg(test)]
#[path = "session_host_forward_tests.rs"]
mod tests;
