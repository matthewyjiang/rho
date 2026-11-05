//! Foreground ACP driver. All terminal writes remain in session::settle.

use super::{
    handshake::{self, error, AgentFeatures, HANDSHAKE_STEP_BUDGET},
    permission::{choose_option, is_allowed},
    policy::AcpAgentPolicy,
    steering,
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
use serde_json::Value;
use std::{
    collections::HashSet,
    future::Future,
    path::Path,
    pin::Pin,
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
    fn stopped(renderer: &EventRenderer, session_id: Option<String>) -> Self {
        Self {
            classification: TurnClassification::Stopped,
            session_id,
            result_text: renderer.result_text().to_owned(),
            turns: renderer.turns(),
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
    // Reborrow so a post-connection cancel outcome can still render.
    let (session_renderer, session_sink) = (&mut *renderer, &mut *sink);
    let policy = Arc::new(Mutex::new(policy));
    let extension_policy = Arc::clone(&policy);
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let extension_tx = events_tx.clone();
    let outcome = Client.builder()
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
            let (renderer, sink) = (session_renderer, session_sink);
            let features = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Ok(DriverOutcome::stopped(renderer, None)),
                result = handshake::initialize(&cx, &policy) => result?,
            };
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
                        _ = cancellation.cancelled() => return Ok(DriverOutcome::stopped(renderer, Some(session.session_id().to_string()))),
                        result = handshake::require_mode(&session, &policy) => result?,
                    }
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => return Ok(DriverOutcome::stopped(renderer, Some(session.session_id().to_string()))),
                        result = handshake::require_config(&session, &policy) => result?,
                    }
                    turns(session, &policy, features, DriverContext { prompt, cwd, cancellation, inbox, renderer, sink }, &events_tx, &mut events_rx).await
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
        }).await;
    match outcome {
        Ok(outcome) => Ok(outcome),
        // Local cancellation is authoritative on every path, not only when a
        // stop reason arrives: an agent that closes stdio, or a failed
        // session/cancel send, still settles as Stopped. Keep the diagnostic.
        Err(source) if cancellation.is_cancelled() => {
            render(
                renderer,
                sink,
                AgentEvent::Notice(format!("acp: after cancellation: {source}")),
            );
            Ok(DriverOutcome::stopped(renderer, None))
        }
        Err(source) => Err(format!("acp: {source}")),
    }
}

/// How long a turn that has ended waits on a steer still in flight. Same
/// 10 s as each ACP setup request; claude-agent-acp answers a steer after one
/// queue check, without waiting on the model.
const STEER_REPLY_BUDGET: std::time::Duration = HANDSHAKE_STEP_BUDGET;

type SteerReply = Pin<Box<dyn Future<Output = Result<Value, Error>> + Send>>;

/// Steering for one session. It takes one parent message at a time from the
/// inbox while a turn runs. A message the agent does not inject waits for the
/// next turn, ahead of anything still queued, and pauses steering to keep the
/// order.
enum Steering {
    /// The agent does not steer, or the inbox closed.
    Off,
    /// Ready for the next parent message.
    Idle,
    /// A `_session/steering` request in flight, with the text it carries.
    InFlight(String, SteerReply),
    /// Not injected; sent as the next turn.
    Declined(String),
}

impl Steering {
    fn take_in_flight(&mut self) -> Option<(String, SteerReply)> {
        match std::mem::replace(self, Self::Idle) {
            Self::InFlight(text, reply) => Some((text, reply)),
            other => {
                *self = other;
                None
            }
        }
    }

    fn take_declined(&mut self) -> Option<String> {
        match std::mem::replace(self, Self::Idle) {
            Self::Declined(text) => Some(text),
            other => {
                *self = other;
                None
            }
        }
    }
}

async fn turns<P: AcpAgentPolicy>(
    mut session: ActiveSession<'_, Agent>,
    policy: &Mutex<P>,
    features: AgentFeatures,
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
    let mut tracking = UpdateTracking::default();
    let mut steering = if features.steering && inbox.is_some() {
        Steering::Idle
    } else {
        Steering::Off
    };
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
                return Ok(DriverOutcome::stopped(renderer, Some(session_id)));
            }
            reply = async { match &mut steering { Steering::InFlight(_, reply) => reply.as_mut().await, _ => std::future::pending().await } }, if matches!(steering, Steering::InFlight(..)) => {
                if let Some((text, _)) = steering.take_in_flight() {
                    steering = settle_steer(text, reply, renderer, sink);
                }
            }
            text = async { inbox.as_mut().expect("steering needs an inbox").recv().await }, if matches!(steering, Steering::Idle) && tracking.turn_live && !cancelled_by_us => match text {
                Some(text) => {
                    let request = steering::request(session.session_id(), &frame_parent_message(&text))?;
                    steering = Steering::InFlight(text, Box::pin(connection.send_request(request).block_task()));
                }
                None => steering = Steering::Off,
            },
            message = session.read_update() => match message? {
                SessionMessage::SessionMessage(dispatch) => dispatch_update(dispatch, policy, renderer, sink, &mut tracking, cancelled_by_us, events_tx).await?,
                SessionMessage::StopReason(stop) => {
                    let classification = classify_stop(&stop, cancelled_by_us);
                    // A steer racing the end of a successful turn is settled
                    // first, so a declined message still becomes the next
                    // turn. Any other ending sends no next turn, so it does not
                    // wait. A cancel during the wait stops the run instead.
                    let mut cancelled_while_settling = false;
                    if let Some((text, reply)) = steering.take_in_flight() {
                        if classification == TurnClassification::Success {
                            tokio::select! {
                                biased;
                                _ = cancellation.cancelled() => cancelled_while_settling = true,
                                reply = tokio::time::timeout(STEER_REPLY_BUDGET, reply) => {
                                    let reply = reply.unwrap_or_else(|_| Err(error(format!("{} reply exceeded budget {} s", steering::METHOD, STEER_REPLY_BUDGET.as_secs()))));
                                    steering = settle_steer(text, reply, renderer, sink);
                                }
                            }
                        }
                    }
                    while let Ok(event) = events_rx.try_recv() { render(renderer, sink, event); }
                    render(renderer, sink, AgentEvent::TurnEnded(stop));
                    if cancelled_while_settling {
                        if let Some(inbox) = inbox.as_ref() { inbox.seal(); }
                        return Ok(DriverOutcome::stopped(renderer, Some(session_id)));
                    }
                    // classify_stop already reports Stopped once we cancelled.
                    if classification == TurnClassification::Success {
                        // Atomic with sends: a racing message either becomes
                        // the next turn (port stays open) or is rejected.
                        if let Some(text) = steering.take_declined().or_else(|| inbox.as_mut().and_then(ParentMessageInbox::take_next_or_close)) {
                            session.send_prompt(frame_parent_message(&text))?;
                            tracking.turn_live = false;
                            sink.apply_effect(StreamEffect::Attachment(AttachmentEvent::Message(Box::new(parent_message_card(text, NotificationDelivery::Queued, "sent as the next agent turn".into())))));
                            continue;
                        }
                    }
                    if let Some(inbox) = inbox.as_ref() { inbox.seal(); }
                    return Ok(DriverOutcome { classification, session_id: Some(session_id), result_text: renderer.result_text().to_owned(), turns: renderer.turns() });
                }
                _ => return Err(error("acp: unhandled session message")),
            },
            Some(event) = events_rx.recv() => render(renderer, sink, event),
            _ = connection.incoming_closed() => return Err(error("acp: connection closed before session/prompt response")),
        }
    }
}

/// Journal an injected message, or keep it for the next turn.
fn settle_steer(
    text: String,
    reply: Result<Value, Error>,
    renderer: &mut EventRenderer,
    sink: &mut StatusSink,
) -> Steering {
    match reply.as_ref().map(steering::delivery) {
        Ok(steering::Delivery::Injected) => {
            sink.apply_effect(StreamEffect::Attachment(AttachmentEvent::Message(
                Box::new(parent_message_card(
                    text,
                    NotificationDelivery::Received,
                    "steered into the running turn".into(),
                )),
            )));
            Steering::Idle
        }
        Ok(steering::Delivery::NextTurn) => Steering::Declined(text),
        Err(source) => {
            render(
                renderer,
                sink,
                AgentEvent::Notice(format!(
                    "acp: {}: {source}; sending the message as the next turn",
                    steering::METHOD
                )),
            );
            Steering::Declined(text)
        }
    }
}

/// What the driver keeps from one session's updates.
#[derive(Default)]
struct UpdateTracking {
    /// Ids only, for the session lifetime: duplicate completed snapshots must
    /// not turn a previously rejected tool back into a successful card.
    rejected: HashSet<ToolCallId>,
    /// The current prompt's turn has produced output. An agent queues a turn
    /// only after its own setup, and declines a steer that arrives first
    /// (claude-agent-acp answers `promptRequired`).
    turn_live: bool,
}

/// Output only a running turn produces. Session-level updates (commands,
/// modes, config, session info) can arrive before the agent has queued it.
fn is_turn_content(update: &SessionUpdate) -> bool {
    match update {
        SessionUpdate::UserMessageChunk(_)
        | SessionUpdate::AgentMessageChunk(_)
        | SessionUpdate::AgentThoughtChunk(_)
        | SessionUpdate::ToolCall(_)
        | SessionUpdate::ToolCallUpdate(_)
        | SessionUpdate::Plan(_)
        | SessionUpdate::UsageUpdate(_) => true,
        SessionUpdate::AvailableCommandsUpdate(_)
        | SessionUpdate::CurrentModeUpdate(_)
        | SessionUpdate::ConfigOptionUpdate(_)
        | SessionUpdate::SessionInfoUpdate(_) => false,
        _ => false,
    }
}

async fn dispatch_update<P: AcpAgentPolicy>(
    dispatch: Dispatch,
    policy: &Mutex<P>,
    renderer: &mut EventRenderer,
    sink: &mut StatusSink,
    tracking: &mut UpdateTracking,
    cancelled_by_us: bool,
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<(), Error> {
    MatchDispatch::new(dispatch)
        .if_notification(async |notification: SessionNotification| {
            let (mut update, required_mode) = {
                let policy = policy.lock().expect("ACP policy");
                (
                    policy.normalize_update(notification.update),
                    policy.session_mode(),
                )
            };
            // A policy may rely on the required mode as part of its fence: an
            // agent that leaves it fails the run instead of continuing
            // unfenced.
            if let (SessionUpdate::CurrentModeUpdate(mode), Some(required)) =
                (&update, &required_mode)
            {
                if mode.current_mode_id != *required {
                    return Err(error(format!(
                        "acp: agent switched to mode `{}`, leaving required mode `{required}`",
                        mode.current_mode_id
                    )));
                }
            }
            tracking.turn_live |= is_turn_content(&update);
            if let SessionUpdate::ToolCallUpdate(call) = &mut update {
                if call.fields.status == Some(ToolCallStatus::Completed)
                    && tracking.rejected.contains(&call.tool_call_id)
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
                tracking
                    .rejected
                    .insert(request.tool_call.tool_call_id.clone());
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
