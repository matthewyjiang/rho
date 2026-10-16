//! The event pump for one prompt: SDK run events, approval requests, and
//! applied background results, forwarded to the host in arrival order.

use std::{future::Future, pin::Pin, sync::atomic::AtomicU64};

use agent_client_protocol::schema::v1::{RequestPermissionOutcome, SessionId, SessionNotification};
use futures_util::stream::{FuturesUnordered, StreamExt};
use rho_sdk::{ApprovalRequestReceiver, CancellationToken, PendingApproval, RunEvent, UserInput};

use super::{persist::TurnCheckpoint, send_notification};
use crate::app::{
    acp::{events::EventMapper, permission, signals::SessionSignals, AcpClientPort},
    headless_delegation::OfferedDisplays,
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
}

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
        mut displays,
    } = sources;
    let mut run_cancelled = cancel.is_cancelled();
    let mut inflight: FuturesUnordered<Pin<Box<dyn Future<Output = ()> + Send + '_>>> =
        FuturesUnordered::new();
    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled(), if !run_cancelled => {
                events.cancel_run();
                run_cancelled = true;
            }

            event = events.next() => {
                let Some(event) = event else {
                    while inflight.next().await.is_some() {}
                    return Ok(());
                };
                if let (RunEvent::BoundaryInputApplied { input, .. }, Some(displays)) =
                    (&event, displays.as_mut())
                {
                    displays.show(session_id, client, input).await?;
                }
                if let Some(notification) = mapper.map_event(session_id, &event) {
                    send_notification(client, notification).await?;
                }
                let signal = signals.as_deref_mut().and_then(|signals| signals.observe(&event));
                if let Some(update) = signal {
                    let notification = SessionNotification::new(session_id.clone(), update);
                    send_notification(client, notification).await?;
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

            Some(()) = inflight.next(), if !inflight.is_empty() => {}
        }
    }
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
