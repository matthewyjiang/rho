//! The event pump for one prompt: SDK run events, approval requests,
//! questions for the user, and applied background results, forwarded to the
//! host in arrival order.

use std::{future::Future, pin::Pin, sync::atomic::AtomicU64};

use agent_client_protocol::schema::v1::{RequestPermissionOutcome, SessionId, SessionNotification};
use futures_util::stream::{FuturesUnordered, StreamExt};
use rho_sdk::{
    ApprovalRequestReceiver, CancellationToken, HostInputId, HostInputRefusal, HostInputRequest,
    HostInputResponse, PendingApproval, RunEvent, ToolCallId, UserInput,
};

use super::{persist::TurnCheckpoint, send_notification};
use crate::{
    app::{
        acp::{
            elicitation,
            events::{EventMapper, RunEnding},
            host_updates::HostUpdates,
            permission,
            signals::SessionSignals,
            AcpClientPort,
        },
        headless_delegation::OfferedDisplays,
    },
    tools::plan_exit::{PlanPermissionReceiver, PlanPermissionRequest},
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
                    tracing::warn!(%error, "could not answer an ACP host input request");
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
    pub(super) session_id: &'a SessionId,
    pub(super) cancel: CancellationToken,
    pub(super) mapper: &'a mut EventMapper,
    /// Session state mirrored alongside events; tests that only exercise the
    /// event and approval plumbing pass `None`.
    pub(super) signals: Option<&'a mut SessionSignals>,
    pub(super) client: &'a dyn AcpClientPort,
    pub(super) placeholders: &'a AtomicU64,
    pub(super) events: &'a mut EventSource<'a>,
    pub(super) approvals: &'a mut ApprovalSource<'a>,
    pub(super) approvals_open: bool,
    /// Plan approvals from `exit_plan_mode`; `None` when the session has no
    /// plan handoff.
    pub(super) plan_approvals: Option<&'a mut PlanPermissionReceiver>,
    /// Whether questions reach the user as form elicitation.
    pub(super) host_updates: HostUpdates,
    /// Background results offered to the run; tests of the event and
    /// approval plumbing pass `None`.
    pub(super) displays: Option<AppliedDisplays<'a>>,
}

/// Background results handed to the model during one prompt. Each is shown
/// where the run applied it and saved with the turn so `session/load`
/// replays it.
pub(super) struct AppliedDisplays<'a> {
    pub(super) offered: OfferedDisplays,
    pub(super) checkpoint: &'a TurnCheckpoint,
}

impl AppliedDisplays<'_> {
    /// Shows and records the rows for an applied boundary input.
    async fn show(
        &mut self,
        session_id: &SessionId,
        client: &dyn AcpClientPort,
        input: &UserInput,
    ) -> anyhow::Result<()> {
        let Some(transcript) = self.offered.applied(input) else {
            return Ok(());
        };
        self.checkpoint
            .record_display(input, transcript.display_message());
        for notification in EventMapper::display_transcript(session_id, &transcript) {
            send_notification(client, notification).await?;
        }
        Ok(())
    }
}

pub(super) async fn pump_sources(sources: PumpSources<'_>) -> anyhow::Result<()> {
    let PumpSources {
        session_id,
        cancel,
        mapper,
        mut signals,
        client,
        placeholders,
        events,
        approvals,
        mut approvals_open,
        mut plan_approvals,
        host_updates,
        mut displays,
    } = sources;
    let mut run_cancelled = cancel.is_cancelled();
    let mut inflight: FuturesUnordered<Pin<Box<dyn Future<Output = ()> + Send + '_>>> =
        FuturesUnordered::new();
    // Questions wait on the user, not on the run, so unlike approvals they
    // are dropped when the run ends, which withdraws them from the host.
    let mut questions: FuturesUnordered<
        Pin<Box<dyn Future<Output = Option<(HostInputId, HostInputAnswer)>> + Send + '_>>,
    > = FuturesUnordered::new();
    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled(), if !run_cancelled => {
                events.cancel_run();
                run_cancelled = true;
            }

            event = events.next() => {
                let Some(event) = event else {
                    drop(questions);
                    for notification in mapper.close_unfinished(session_id, RunEnding::WithoutResult) {
                        send_notification(client, notification).await?;
                    }
                    while inflight.next().await.is_some() {}
                    return Ok(());
                };
                if let (RunEvent::BoundaryInputApplied { input, .. }, Some(displays)) =
                    (&event, displays.as_mut())
                {
                    displays.show(session_id, client, input).await?;
                }
                for notification in mapper.map_event(session_id, &event) {
                    send_notification(client, notification).await?;
                }
                let signal = match signals.as_deref_mut() {
                    Some(signals) => signals.observe(&event).await,
                    None => None,
                };
                if let Some(update) = signal {
                    let notification = SessionNotification::new(session_id.clone(), update);
                    send_notification(client, notification).await?;
                }
                let asked = match event {
                    RunEvent::HostInputRequested { request } => Some((request, None)),
                    RunEvent::ToolHostInputRequested { call_id, request } => {
                        Some((request, Some(call_id)))
                    }
                    _ => None,
                };
                if let Some((request, call_id)) = asked {
                    if host_updates.form_elicitation {
                        questions.push(Box::pin(ask_user(
                            session_id.clone(),
                            request,
                            call_id,
                            client,
                            cancel.clone(),
                        )));
                    } else {
                        // Rho offers no asking tools to such hosts; anything
                        // that asks anyway must not wait forever.
                        events.answer(request.id().clone(), Err(HostInputRefusal::Declined));
                    }
                }
            }

            pending = approvals.next(), if approvals_open => {
                let Some(pending) = pending else {
                    approvals_open = false;
                    continue;
                };
                inflight.push(Box::pin(answer_approval(
                    session_id.clone(),
                    pending,
                    client,
                    cancel.clone(),
                    placeholders,
                )));
            }

            request = next_plan_approval(plan_approvals.as_deref_mut()) => {
                match request {
                    Some(request) => questions.push(Box::pin(answer_plan_approval(
                        session_id.clone(),
                        request,
                        client,
                        cancel.clone(),
                    ))),
                    None => plan_approvals = None,
                }
            }

            Some(()) = inflight.next(), if !inflight.is_empty() => {}

            Some(answered) = questions.next(), if !questions.is_empty() => {
                if let Some((id, answer)) = answered {
                    events.answer(id, answer);
                }
            }
        }
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

/// Puts a host input request to the user as a form. A cancelled prompt
/// withdraws the form and leaves the run's cancellation to end the request.
async fn ask_user(
    session_id: SessionId,
    request: HostInputRequest,
    call_id: Option<ToolCallId>,
    client: &dyn AcpClientPort,
    cancel: CancellationToken,
) -> Option<(HostInputId, HostInputAnswer)> {
    let form = elicitation::elicitation_request(&request, &session_id, call_id.as_ref());
    let answer = tokio::select! {
        biased;
        () = cancel.cancelled() => return None,
        response = client.create_elicitation(form) => match response {
            Ok(response) => elicitation::host_answer(&request, response),
            Err(error) => {
                tracing::warn!(%error, "ACP host failed an elicitation request");
                Err(HostInputRefusal::Dismissed)
            }
        },
    };
    Some((request.id().clone(), answer))
}

/// Asks the host to approve a plan. A cancelled prompt, or a tool that
/// stopped waiting, withdraws the prompt.
async fn answer_plan_approval(
    session_id: SessionId,
    mut request: PlanPermissionRequest,
    client: &dyn AcpClientPort,
    cancel: CancellationToken,
) -> Option<(HostInputId, HostInputAnswer)> {
    let prompt = permission::plan_approval_request(&session_id, &request);
    let outcome = tokio::select! {
        biased;
        () = cancel.cancelled() => return None,
        () = request.reply.closed() => return None,
        response = client.request_permission(prompt) => match response {
            Ok(response) => response.outcome,
            Err(error) => {
                tracing::warn!(%error, "ACP host failed a plan approval prompt");
                RequestPermissionOutcome::Cancelled
            }
        },
    };
    let answer = permission::plan_answer_for(&outcome, &request.targets);
    let _ = request.reply.send(answer);
    None
}

async fn answer_approval(
    session_id: SessionId,
    mut pending: PendingApproval,
    client: &dyn AcpClientPort,
    cancel: CancellationToken,
    placeholders: &AtomicU64,
) {
    let placeholder = permission::next_placeholder_tool_call_id(placeholders);
    let request = permission::permission_request(&session_id, pending.request(), &placeholder);
    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => RequestPermissionOutcome::Cancelled,
        response = client.request_permission(request) => match response {
            Ok(response) => response.outcome,
            Err(_) => RequestPermissionOutcome::Cancelled,
        },
    };
    let _ = pending.respond(permission::decision_for(&outcome));
}
