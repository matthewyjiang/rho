//! Event pump for runs without a local TUI attached.
//!
//! The pump owns three concerns that would otherwise tangle the automation
//! entry points: draining [`rho_sdk::RunEvent`]s, answering host
//! questionnaires through a [`HostInputResponder`], and keeping the status file
//! fresh with reporter heartbeats. Every failure path leaves the run cancelled
//! and awaited, so bounded event channels cannot deadlock the worker.

use std::{collections::VecDeque, future::Future, pin::Pin};

use super::{
    automation::{emit, RunReporter},
    automation_protocol::JsonlAdapter,
};

/// Future returned by [`HostInputResponder`] implementations.
pub(crate) type HostInputRespondFuture<'a> =
    Pin<Box<dyn Future<Output = Result<rho_sdk::HostInputResponse, rho_sdk::Error>> + Send + 'a>>;

type HostInputAckFuture = Pin<Box<dyn Future<Output = Result<(), rho_sdk::Error>> + Send>>;

/// Keeps the status file fresh while a provider or tool call emits no events.
const REPORT_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(10);

/// Answers structured host questionnaires for a headless automation run.
///
/// Direct CLI automation leaves this unset and fails closed. Interactive hosts
/// supply an implementation that forwards requests to a parent session or UI.
pub(crate) trait HostInputResponder: Send + Sync {
    fn respond<'a>(
        &'a self,
        request: rho_sdk::HostInputRequest,
        cancellation: &'a rho_sdk::CancellationToken,
    ) -> HostInputRespondFuture<'a>;
}

pub(crate) struct HeadlessRunDeps<'a> {
    pub(crate) reporter: Option<&'a mut RunReporter>,
    pub(crate) external_cancellation: Option<rho_tools::cancellation::RunCancellation>,
    pub(crate) jsonl: Option<&'a mut JsonlAdapter>,
    pub(crate) host_input: Option<&'a dyn HostInputResponder>,
}

/// Drives a run to its outcome, cancelling the run before returning any error.
pub(crate) async fn drive(
    run: &mut rho_sdk::Run,
    reporter: Option<&mut RunReporter>,
    jsonl: Option<&mut JsonlAdapter>,
    host_input: Option<&dyn HostInputResponder>,
) -> anyhow::Result<rho_sdk::RunOutcome> {
    match pump(run, reporter, jsonl, host_input).await {
        Ok(()) => Ok(run.outcome().await?),
        Err(error) => {
            run.cancel();
            let _ = run.outcome().await;
            Err(error)
        }
    }
}

/// Pumps events until the run closes its event stream and no host question is
/// outstanding. Returns on the first failure and leaves cleanup to [`drive`].
async fn pump(
    run: &mut rho_sdk::Run,
    mut reporter: Option<&mut RunReporter>,
    mut jsonl: Option<&mut JsonlAdapter>,
    host_input: Option<&dyn HostInputResponder>,
) -> anyhow::Result<()> {
    let cancellation = run.cancellation_handle();
    let mut heartbeat = tokio::time::interval(REPORT_HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pending_requests: VecDeque<rho_sdk::HostInputRequest> = VecDeque::new();
    let mut parent_wait: Option<AnsweringRequest<'_>> = None;
    let mut ack_wait: Option<HostInputAckFuture> = None;
    let mut events_open = true;

    loop {
        if parent_wait.is_none() && ack_wait.is_none() {
            if let Some(request) = pending_requests.pop_front() {
                let Some(responder) = host_input else {
                    anyhow::bail!(
                        "rho run cannot answer host input request '{}' ({}); run without tools that require interactive input",
                        request.id(),
                        request.title(),
                    );
                };
                let id = request.id().clone();
                parent_wait = Some(AnsweringRequest {
                    id,
                    respond: responder.respond(request, &cancellation),
                });
            }
        }

        if !events_open
            && parent_wait.is_none()
            && ack_wait.is_none()
            && pending_requests.is_empty()
        {
            return Ok(());
        }

        tokio::select! {
            biased;

            answer = settle_answer(&mut parent_wait) => {
                let (id, response) = answer;
                ack_wait = Some(Box::pin(run.request_respond(id, response?)?));
            }

            ack = settle(&mut ack_wait) => ack?,

            event = run.next_event(), if events_open => {
                let Some(event) = event else {
                    events_open = false;
                    continue;
                };
                if let Some(reporter) = reporter.as_deref_mut() {
                    reporter.on_event(&event);
                }
                if let Some(adapter) = jsonl.as_deref_mut() {
                    if let Some(wire_event) = adapter.event(&event) {
                        emit(wire_event)?;
                    }
                }
                match event {
                    rho_sdk::RunEvent::HostInputRequested { request }
                    | rho_sdk::RunEvent::ToolHostInputRequested { request, .. } => {
                        pending_requests.push_back(request);
                    }
                    _ => {}
                }
            }

            _ = heartbeat.tick(), if reporter.is_some() => {
                if let Some(reporter) = reporter.as_deref_mut() {
                    reporter.write();
                }
            }
        }
    }
}

/// One host question waiting on an answer from the responder.
struct AnsweringRequest<'a> {
    id: rho_sdk::HostInputId,
    respond: HostInputRespondFuture<'a>,
}

/// Awaits the outstanding answer and clears the slot, so the caller never has
/// to re-check whether the slot was occupied.
async fn settle_answer(
    slot: &mut Option<AnsweringRequest<'_>>,
) -> (
    rho_sdk::HostInputId,
    Result<rho_sdk::HostInputResponse, rho_sdk::Error>,
) {
    let Some(pending) = slot.as_mut() else {
        return std::future::pending().await;
    };
    let response = pending.respond.as_mut().await;
    let id = pending.id.clone();
    *slot = None;
    (id, response)
}

/// Awaits an optional future and clears the slot. Stays pending forever while
/// the slot is empty, so `select!` needs no guard and no unwrap.
async fn settle<F: Future + ?Sized>(slot: &mut Option<Pin<Box<F>>>) -> F::Output {
    let Some(future) = slot.as_mut() else {
        return std::future::pending().await;
    };
    let output = future.as_mut().await;
    *slot = None;
    output
}
