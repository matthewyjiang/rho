//! Foreground ACP driver. All terminal writes remain in session::settle.

use super::{
    handshake::{self, error, HANDSHAKE_STEP_BUDGET},
    permission::{choose_option, is_allowed},
    policy::AcpAgentPolicy,
    turn::{classify_stop, TurnClassification},
    ExtensionAnswer,
};
use crate::cli_runtime::{
    agent_event::{render::EventRenderer, AgentEvent},
    parent_messages::{frame_parent_message, ParentMessageInbox},
    status_sink::StatusSink,
    stream_effect::{StatusPatch, StreamEffect},
};
use crate::{
    presentation::{parent_message_card, NotificationDelivery},
    run_artifacts::AttachmentEvent,
};
use agent_client_protocol::util::MatchDispatch;
use agent_client_protocol::{
    schema::v1::{
        CancelNotification, NewSessionRequest, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, SessionNotification, SessionUpdate, ToolCallContent, ToolCallId,
        ToolCallStatus,
    },
    ActiveSession, Agent, Client, ConnectTo, Dispatch, Error, Handled, Responder, SessionMessage,
    UntypedMessage,
};
use rho_tools::cancellation::RunCancellation;
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

// Receipt: OwnedChild's 200 ms termination grace. A cooperative cancel was
// observed at ~4 ms in the spike; share the existing shutdown budget before
// forcing termination, rather than leave a hung agent immortal after cancel.
const CANCEL_DRAIN_BUDGET: Duration = Duration::from_millis(200);

pub(super) struct DriverOutcome {
    pub(super) classification: TurnClassification,
    pub(super) session_id: Option<String>,
    pub(super) result_text: String,
    pub(super) turns: u64,
}

impl DriverOutcome {
    fn stopped(renderer: &EventRenderer, session_id: Option<String>, turns: u64) -> Self {
        Self {
            classification: TurnClassification::Stopped,
            session_id,
            result_text: renderer.result_text().to_owned(),
            turns,
        }
    }
}

pub(super) fn render(renderer: &mut EventRenderer, sink: &mut StatusSink, event: AgentEvent) {
    for effect in renderer.render(event) {
        sink.apply_effect(effect);
    }
}

fn answer_extension<P: AcpAgentPolicy>(
    policy: &Mutex<P>,
    request: UntypedMessage,
    responder: Responder<serde_json::Value>,
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<(), Error> {
    let answer = policy
        .lock()
        .expect("ACP policy")
        .answer_extension(&request.method, &request.params);
    match answer {
        Some(ExtensionAnswer {
            reply,
            events: presentation,
        }) => {
            responder.respond(reply)?;
            for event in presentation {
                let _ = events.send(event);
            }
            Ok(())
        }
        None => responder.respond_with_error(Error::method_not_found()),
    }
}

pub(super) struct DriverContext<'a> {
    pub(super) prompt: &'a str,
    pub(super) cwd: &'a Path,
    pub(super) cancellation: &'a RunCancellation,
    pub(super) inbox: &'a mut Option<ParentMessageInbox>,
    pub(super) renderer: &'a mut EventRenderer,
    pub(super) sink: &'a mut StatusSink,
}

pub(super) async fn run<P: AcpAgentPolicy>(
    transport: impl ConnectTo<Client> + 'static,
    policy: P,
    context: DriverContext<'_>,
) -> Result<DriverOutcome, String> {
    let DriverContext {
        prompt,
        cwd,
        cancellation,
        inbox,
        renderer,
        sink,
    } = context;
    let policy = Arc::new(Mutex::new(policy));
    let extension_policy = Arc::clone(&policy);
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let extension_tx = events_tx.clone();
    Client.builder()
        .name("rho")
        .on_receive_request(async move |request: UntypedMessage, responder, _cx| {
            // Static handlers run BEFORE ActiveSession's dynamic handler.
            // Decline all session-scoped requests so permissions and scoped
            // extensions arrive through read_update, never this catch-all.
            if request.params.get("sessionId").and_then(serde_json::Value::as_str).is_some() {
                return Ok(Handled::No { message: (request, responder), retry: false });
            }
            answer_extension(&extension_policy, request, responder, &extension_tx)?;
            Ok(Handled::Yes)
        }, agent_client_protocol::on_receive_request!())
        .connect_with(transport, async move |cx| {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Ok(DriverOutcome::stopped(renderer, None, 0)),
                result = handshake::initialize(&cx, &policy) => result?,
            }
            let (started_tx, started_rx) = oneshot::channel();
            let started = std::sync::atomic::AtomicBool::new(false);
            // run_until preserves session/new's actual JSON-RPC error (unlike
            // start_session, which starts a background request task). Bound
            // only startup: after the closure starts, turns have no deadline.
            let session_run = cx.build_session_from(NewSessionRequest::new(cwd.to_path_buf()))
                .block_task().run_until(async |session| {
                    started.store(true, std::sync::atomic::Ordering::Relaxed);
                    let _ = started_tx.send(());
                    sink.apply_effect(StreamEffect::Status(StatusPatch {
                        claude_session_id: Some(session.session_id().to_string()),
                        ..StatusPatch::default()
                    }));
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => return Ok(DriverOutcome::stopped(renderer, Some(session.session_id().to_string()), 0)),
                        result = handshake::require_mode(&session, &policy) => result?,
                    }
                    turns(session, &policy, DriverContext { prompt, cwd, cancellation, inbox, renderer, sink }, &events_tx, &mut events_rx).await
                });
            tokio::pin!(session_run);
            tokio::select! {
                biased;
                started_result = tokio::time::timeout(HANDSHAKE_STEP_BUDGET, started_rx) => {
                    match started_result {
                        Ok(Ok(())) => session_run.await,
                        Ok(Err(_)) => session_run.await.map_err(|source| error(format!("acp: session/new: {source}"))),
                        Err(_) => Err(error("acp: session/new exceeded handshake step budget 10 s")),
                    }
                }
                _ = cancellation.cancelled() => Ok(DriverOutcome { classification: TurnClassification::Stopped, session_id: None, result_text: String::new(), turns: 0 }),
                result = &mut session_run => result.map_err(|source| if started.load(std::sync::atomic::Ordering::Relaxed) { source } else { error(format!("acp: session/new: {source}")) }),
            }
        }).await.map_err(|source| format!("acp: {source}"))
}

async fn turns<P: AcpAgentPolicy>(
    mut session: ActiveSession<'_, Agent>,
    policy: &Mutex<P>,
    context: DriverContext<'_>,
    events_tx: &mpsc::UnboundedSender<AgentEvent>,
    events_rx: &mut mpsc::UnboundedReceiver<AgentEvent>,
) -> Result<DriverOutcome, Error> {
    let DriverContext {
        prompt,
        cancellation,
        inbox,
        renderer,
        sink,
        ..
    } = context;
    let session_id = session.session_id().to_string();
    sink.mark_running();
    session.send_prompt(prompt)?;
    let connection = session.connection().clone();
    let mut cancelled_by_us = false;
    let mut cancel_deadline = None;
    let mut turns = 0;
    // Keep only ids, for the session lifetime: duplicate completed snapshots
    // must not turn a previously rejected tool back into a successful card.
    let mut rejected = HashSet::new();
    loop {
        tokio::select! {
            biased;
            _ = cancellation.cancelled(), if !cancelled_by_us => {
                connection.send_notification(CancelNotification::new(session.session_id().clone()))?;
                cancelled_by_us = true;
                cancel_deadline = Some(tokio::time::Instant::now() + CANCEL_DRAIN_BUDGET);
                if let Some(inbox) = inbox.as_ref() { inbox.seal(); }
            }
            _ = async { match cancel_deadline { Some(deadline) => tokio::time::sleep_until(deadline).await, None => std::future::pending().await } } => {
                render(renderer, sink, AgentEvent::Notice("acp: session/cancel response exceeded shutdown grace budget 200 ms; terminating agent".into()));
                return Ok(DriverOutcome::stopped(renderer, Some(session_id), turns));
            }
            message = session.read_update() => match message? {
                SessionMessage::SessionMessage(dispatch) => dispatch_update(dispatch, policy, renderer, sink, &mut rejected, cancelled_by_us, events_tx).await?,
                SessionMessage::StopReason(stop) => {
                    turns += 1;
                    while let Ok(event) = events_rx.try_recv() { render(renderer, sink, event); }
                    render(renderer, sink, AgentEvent::TurnEnded(stop));
                    let classification = classify_stop(&stop, cancelled_by_us);
                    // classify_stop already reports Stopped once we cancelled.
                    if classification == TurnClassification::Success {
                        // Atomic with sends: a racing message either becomes
                        // the next turn (port stays open) or is rejected.
                        if let Some(text) = inbox.as_mut().and_then(ParentMessageInbox::take_next_or_close) {
                            session.send_prompt(frame_parent_message(&text))?;
                            sink.apply_effect(StreamEffect::Attachment(AttachmentEvent::Message(Box::new(parent_message_card(text, NotificationDelivery::Queued, "sent as the next agent turn".into())))));
                            continue;
                        }
                    }
                    if let Some(inbox) = inbox.as_ref() { inbox.seal(); }
                    return Ok(DriverOutcome { classification, session_id: Some(session_id), result_text: renderer.result_text().to_owned(), turns });
                }
                _ => return Err(error("acp: unhandled session message")),
            },
            Some(event) = events_rx.recv() => render(renderer, sink, event),
            _ = connection.incoming_closed() => return Err(error("acp: connection closed before session/prompt response")),
        }
    }
}

async fn dispatch_update<P: AcpAgentPolicy>(
    dispatch: Dispatch,
    policy: &Mutex<P>,
    renderer: &mut EventRenderer,
    sink: &mut StatusSink,
    rejected: &mut HashSet<ToolCallId>,
    cancelled_by_us: bool,
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<(), Error> {
    MatchDispatch::new(dispatch)
        .if_notification(async |notification: SessionNotification| {
            let mut update = policy
                .lock()
                .expect("ACP policy")
                .normalize_update(notification.update);
            if let SessionUpdate::ToolCallUpdate(call) = &mut update {
                if call.fields.status == Some(ToolCallStatus::Completed)
                    && rejected.contains(&call.tool_call_id)
                {
                    call.fields.status = Some(ToolCallStatus::Failed);
                    call.fields.content = Some(vec![ToolCallContent::from(
                        "rejected by Rho permission policy",
                    )]);
                }
            }
            render(renderer, sink, AgentEvent::from(update));
            Ok(())
        })
        .await
        .if_request(async |request: RequestPermissionRequest, responder| {
            let outcome = if cancelled_by_us {
                RequestPermissionOutcome::Cancelled
            } else {
                choose_option(
                    policy
                        .lock()
                        .expect("ACP policy")
                        .decide_permission(&request),
                    &request.options,
                )
            };
            if !is_allowed(&outcome, &request.options) {
                rejected.insert(request.tool_call.tool_call_id.clone());
            }
            responder.respond(RequestPermissionResponse::new(outcome))
        })
        .await
        .if_request(async |request: UntypedMessage, responder| {
            answer_extension(policy, request, responder, events)
        })
        .await
        .otherwise_ignore()
}
