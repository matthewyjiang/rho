use std::{
    collections::BTreeMap, future::Future, num::NonZeroUsize, pin::Pin, sync::Arc, task::Poll,
    time::Instant,
};

use tokio::sync::mpsc;

use crate::{
    event::{ToolCompletion, ToolFailure},
    host_input::HostInputEnvelope,
    model::{Message, ToolCall, ToolResult},
    run::RunCommand,
    session::{SessionCore, SessionState},
    tool::{
        PreparedToolInvocation, ToolContext, ToolError, ToolErrorKind, ToolExecutionPolicy,
        ToolFuture, ToolInvocationSource, ToolOutput, ToolProgress,
    },
    CancellationToken, Error, HostInputId, RunEvent, ToolCallId,
};

mod preparation;

use super::planner::{plan, Dependency};
use crate::orchestration::{emit, Rho, RunControl};
use preparation::prepare_batch;

pub(in crate::orchestration) const INTERRUPTED_TOOL_RESULT_CONTENT: &str =
    "tool call interrupted before completion";

type AuthorizationFuture<'a> = Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>>;
type ExecutionFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>>;

enum CallState<'a> {
    Unavailable,
    PreparationFailed(ToolError),
    Prepared {
        invocation: Option<PreparedToolInvocation<'a>>,
        context: ToolContext,
    },
    Authorizing {
        invocation: Option<PreparedToolInvocation<'a>>,
        context: ToolContext,
        future: AuthorizationFuture<'a>,
    },
    Ready {
        invocation: Option<PreparedToolInvocation<'a>>,
        context: ToolContext,
    },
    Running(ExecutionFuture<'a>),
    Finishing(Option<Result<ToolOutput, ToolError>>),
    Resolved,
}

struct BatchCall<'a> {
    call: ToolCall,
    id: ToolCallId,
    state: CallState<'a>,
    progress: Option<crate::tool::ToolProgressReceiver>,
    host_input: Option<mpsc::Receiver<HostInputEnvelope>>,
    dependencies: Vec<Dependency>,
    queued_at: Instant,
    execution_started: Option<Instant>,
    result: Option<ToolResult>,
}

enum WorkerEvent {
    Progress {
        index: usize,
        progress: ToolProgress,
    },
    HostInput {
        index: usize,
        request: HostInputEnvelope,
    },
}

enum NextEvent {
    Authorized {
        index: usize,
        result: Result<(), ToolError>,
    },
    Completed {
        index: usize,
        result: Result<ToolOutput, ToolError>,
    },
    Worker(WorkerEvent),
    Command(Option<RunCommand>),
    Cancelled,
}

pub(in crate::orchestration) async fn execute(
    core: &Arc<SessionCore>,
    runtime: &Rho,
    calls: Vec<(ToolCall, ToolCallId, ToolInvocationSource)>,
    history: &mut Vec<Message>,
    control: &mut RunControl<'_>,
) -> Result<bool, Error> {
    let limit = runtime.max_parallel_tools;
    let batch_span = tracing::info_span!(
        "tool_batch",
        batch_size = calls.len(),
        max_parallel_tools = limit.get()
    );
    batch_span.in_scope(|| tracing::trace!("batch coordinator started"));
    let batch_cancellation = control.cancellation.clone();
    if let Err(error) = propose_calls(control, &calls).await {
        batch_cancellation.cancel();
        append_interrupted_calls(&calls, history);
        return if matches!(error, Error::Cancelled) {
            Ok(true)
        } else {
            Err(error)
        };
    }
    let tools = calls
        .iter()
        .map(|(call, _, _)| runtime.tools.get(&call.name))
        .collect::<Vec<_>>();
    let (worker_tx, mut worker_rx) = mpsc::channel(limit.get());
    let (mut batch, preparation_cancelled) =
        prepare_batch(core, runtime, &tools, calls, &batch_cancellation, limit).await;
    if preparation_cancelled {
        interrupt_batch(&batch_cancellation, &mut batch, history);
        return Ok(true);
    }

    let policies = batch
        .iter()
        .map(|entry| match &entry.state {
            CallState::Prepared { invocation, .. } => invocation
                .as_ref()
                .expect("prepared invocation is present")
                .execution_policy()
                .clone(),
            _ => ToolExecutionPolicy::resource_aware([]),
        })
        .collect::<Vec<_>>();
    for planned in plan(&policies) {
        for dependency in &planned.dependencies {
            tracing::trace!(
                call_index = planned.index.0,
                predecessor_index = dependency.predecessor.0,
                serialization_reason = ?dependency.reason,
                "tool call serialized"
            );
        }
        batch[planned.index.0].dependencies = planned.dependencies;
    }
    let mut pending_input: BTreeMap<HostInputId, (ToolCallId, HostInputEnvelope)> = BTreeMap::new();
    let mut observed_input: BTreeMap<HostInputId, ToolCallId> = BTreeMap::new();
    let mut commands_open = true;
    let mut peak_running = 0;
    loop {
        if let Err(error) = resolve_without_work(control, &mut batch).await {
            return cancel_batch(
                error,
                &batch_cancellation,
                &mut batch,
                &mut worker_rx,
                history,
            )
            .await;
        }
        if let Err(error) = start_eligible(control, &mut batch).await {
            return cancel_batch(
                error,
                &batch_cancellation,
                &mut batch,
                &mut worker_rx,
                history,
            )
            .await;
        }
        peak_running = peak_running.max(start_ready(
            &mut batch,
            limit,
            worker_tx.clone(),
            batch_cancellation.clone(),
        ));

        if batch
            .iter()
            .all(|entry| matches!(entry.state, CallState::Resolved))
        {
            append_results(&mut batch, history);
            tracing::debug!(peak_parallel_tools = peak_running, "tool batch completed");
            core.set_state(SessionState::Running);
            return Ok(false);
        }

        let next = std::future::poll_fn(|cx| {
            // Model-order polling makes a ready completion win over cancellation
            // when both become observable in this poll.
            for (index, entry) in batch.iter_mut().enumerate() {
                match &mut entry.state {
                    CallState::Authorizing { future, .. } => {
                        if let Poll::Ready(result) = future.as_mut().poll(cx) {
                            return Poll::Ready(NextEvent::Authorized { index, result });
                        }
                    }
                    CallState::Running(future) => {
                        if let Poll::Ready(result) = future.as_mut().poll(cx) {
                            entry.state = CallState::Finishing(Some(result));
                        }
                    }
                    _ => {}
                }
            }
            if commands_open {
                if let Poll::Ready(command) = control.commands.poll_recv(cx) {
                    return Poll::Ready(NextEvent::Command(command));
                }
            }
            if let Poll::Ready(Some(event)) = worker_rx.poll_recv(cx) {
                return Poll::Ready(NextEvent::Worker(event));
            }
            for (index, entry) in batch.iter_mut().enumerate() {
                if let CallState::Finishing(result) = &mut entry.state {
                    return Poll::Ready(NextEvent::Completed {
                        index,
                        result: result.take().expect("finishing call has a result"),
                    });
                }
            }
            let cancellation = control.cancellation.cancelled();
            tokio::pin!(cancellation);
            if cancellation.poll(cx).is_ready() {
                return Poll::Ready(NextEvent::Cancelled);
            }
            Poll::Pending
        })
        .await;

        if matches!(&next, NextEvent::Command(None)) {
            commands_open = false;
        }
        match handle_next(
            next,
            core,
            control,
            &mut batch,
            &mut pending_input,
            &mut observed_input,
            &mut commands_open,
        )
        .await
        {
            Ok(true) => {
                settle_running_after_cancellation(&batch_cancellation, &mut batch, &mut worker_rx)
                    .await;
                interrupt_batch(&batch_cancellation, &mut batch, history);
                return Ok(true);
            }
            Ok(false) => {}
            Err(error) => {
                return cancel_batch(
                    error,
                    &batch_cancellation,
                    &mut batch,
                    &mut worker_rx,
                    history,
                )
                .await;
            }
        }
    }
}

async fn propose_calls(
    control: &mut RunControl<'_>,
    calls: &[(ToolCall, ToolCallId, ToolInvocationSource)],
) -> Result<(), Error> {
    for (call, _, _) in calls {
        emit(
            control.events,
            control.cancellation,
            RunEvent::ToolProposed { call: call.clone() },
        )
        .await?;
    }
    Ok(())
}

fn append_interrupted_calls(
    calls: &[(ToolCall, ToolCallId, ToolInvocationSource)],
    history: &mut Vec<Message>,
) {
    history.extend(calls.iter().map(|(call, _, _)| {
        Message::ToolResult(ToolResult {
            id: call.id.clone(),
            ok: false,
            content: INTERRUPTED_TOOL_RESULT_CONTENT.into(),
        })
    }));
}

async fn resolve_without_work(
    control: &mut RunControl<'_>,
    batch: &mut [BatchCall<'_>],
) -> Result<(), Error> {
    for entry in batch {
        let completion = match &entry.state {
            CallState::Unavailable => Some((
                ToolCompletion::Unavailable,
                ToolResult {
                    id: entry.call.id.clone(),
                    ok: false,
                    content: format!("tool '{}' is unavailable", entry.call.name),
                },
            )),
            CallState::PreparationFailed(error) => {
                emit(
                    control.events,
                    control.cancellation,
                    RunEvent::ToolStarted {
                        call_id: entry.id.clone(),
                        name: entry.call.name.clone(),
                        metadata: Default::default(),
                    },
                )
                .await?;
                Some((
                    ToolCompletion::Failure(ToolFailure::new(
                        error.kind(),
                        error.message().to_owned(),
                    )),
                    failed_result(&entry.call, error),
                ))
            }
            _ => None,
        };
        let Some((completion, result)) = completion else {
            continue;
        };
        entry.result = Some(result);
        entry.state = CallState::Resolved;
        emit(
            control.events,
            control.cancellation,
            RunEvent::ToolFinished {
                call_id: entry.id.clone(),
                result: completion,
            },
        )
        .await?;
    }
    Ok(())
}

async fn start_eligible(
    control: &mut RunControl<'_>,
    batch: &mut [BatchCall<'_>],
) -> Result<(), Error> {
    for index in 0..batch.len() {
        let eligible = matches!(batch[index].state, CallState::Prepared { .. })
            && batch[index].dependencies.iter().all(|dependency| {
                matches!(batch[dependency.predecessor.0].state, CallState::Resolved)
            });
        if !eligible {
            continue;
        }
        let (metadata, capabilities) = match &batch[index].state {
            CallState::Prepared { invocation, .. } => {
                let prepared = invocation.as_ref().expect("prepared invocation is present");
                (
                    prepared.start_metadata().clone(),
                    prepared.capabilities().to_vec(),
                )
            }
            _ => unreachable!(),
        };
        emit(
            control.events,
            control.cancellation,
            RunEvent::ToolStarted {
                call_id: batch[index].id.clone(),
                name: batch[index].call.name.clone(),
                metadata,
            },
        )
        .await?;
        let CallState::Prepared {
            invocation,
            context,
        } = std::mem::replace(&mut batch[index].state, CallState::Resolved)
        else {
            unreachable!()
        };
        tracing::debug!(call_index = index, "tool call entered authorization");
        if capabilities.is_empty() {
            batch[index].state = CallState::Ready {
                invocation,
                context,
            };
            continue;
        }
        let authorization_context = context.clone();
        let future = Box::pin(async move {
            for capability in capabilities {
                authorization_context
                    .authorize(capability)
                    .await
                    .map_err(|error| {
                        if matches!(error.kind(), crate::AuthorizationDenialKind::Cancelled) {
                            ToolError::cancelled()
                        } else {
                            ToolError::policy_denied(&error)
                        }
                    })?;
            }
            Ok(())
        });
        batch[index].state = CallState::Authorizing {
            invocation,
            context,
            future,
        };
    }
    Ok(())
}

fn start_ready<'a>(
    batch: &mut [BatchCall<'a>],
    limit: NonZeroUsize,
    worker_tx: mpsc::Sender<WorkerEvent>,
    cancellation: CancellationToken,
) -> usize {
    let mut running = batch
        .iter()
        .filter(|entry| matches!(entry.state, CallState::Running(_)))
        .count();
    for (index, entry) in batch.iter_mut().enumerate() {
        if cancellation.is_cancelled() || running >= limit.get() {
            break;
        }
        if !matches!(entry.state, CallState::Ready { .. }) {
            continue;
        }
        let CallState::Ready {
            mut invocation,
            context,
        } = std::mem::replace(&mut entry.state, CallState::Resolved)
        else {
            unreachable!()
        };
        let future = invocation
            .take()
            .expect("ready invocation is present")
            .execute(context);
        let progress = entry
            .progress
            .take()
            .expect("ready call has a progress receiver");
        let host_input = entry
            .host_input
            .take()
            .expect("ready call has a host-input receiver");
        entry.state = CallState::Running(forward_execution(
            index,
            future,
            progress,
            host_input,
            worker_tx.clone(),
            cancellation.clone(),
        ));
        entry.execution_started = Some(Instant::now());
        running += 1;
        tracing::debug!(
            call_index = index,
            active_workers = running,
            queue_duration_ms = entry.queued_at.elapsed().as_millis() as u64,
            "tool call entered execution"
        );
    }
    running
}

fn forward_execution<'a>(
    index: usize,
    future: ToolFuture<'a>,
    mut progress: crate::tool::ToolProgressReceiver,
    mut host_input: mpsc::Receiver<HostInputEnvelope>,
    events: mpsc::Sender<WorkerEvent>,
    cancellation: CancellationToken,
) -> ExecutionFuture<'a> {
    Box::pin(async move {
        if cancellation.is_cancelled() {
            return Err(ToolError::cancelled());
        }
        tokio::pin!(future);
        let mut progress_open = true;
        let mut host_input_open = true;
        loop {
            tokio::select! {
                biased;
                result = &mut future => {
                    while let Some(progress) = progress.try_recv() {
                        tokio::select! {
                            biased;
                            sent = events.send(WorkerEvent::Progress { index, progress }) => {
                                if sent.is_err() {
                                    return result;
                                }
                            }
                            () = cancellation.cancelled() => return result,
                        }
                    }
                    return result;
                },
                () = cancellation.cancelled() => return Err(ToolError::cancelled()),
                update = progress.recv(), if progress_open => match update {
                    Some(progress) => {
                        if events.send(WorkerEvent::Progress { index, progress }).await.is_err() {
                            return Err(ToolError::cancelled());
                        }
                    }
                    None => progress_open = false,
                },
                request = host_input.recv(), if host_input_open => match request {
                    Some(request) => {
                        if events.send(WorkerEvent::HostInput { index, request }).await.is_err() {
                            return Err(ToolError::cancelled());
                        }
                    }
                    None => host_input_open = false,
                },
            }
        }
    })
}

async fn handle_next(
    next: NextEvent,
    core: &Arc<SessionCore>,
    control: &mut RunControl<'_>,
    batch: &mut [BatchCall<'_>],
    pending_input: &mut BTreeMap<HostInputId, (ToolCallId, HostInputEnvelope)>,
    observed_input: &mut BTreeMap<HostInputId, ToolCallId>,
    commands_open: &mut bool,
) -> Result<bool, Error> {
    match next {
        NextEvent::Authorized { index, result } => {
            let CallState::Authorizing {
                invocation,
                context,
                ..
            } = std::mem::replace(&mut batch[index].state, CallState::Resolved)
            else {
                unreachable!()
            };
            match result {
                Ok(()) if control.cancellation.is_cancelled() => {
                    batch[index].state = CallState::PreparationFailed(ToolError::cancelled());
                    return Ok(true);
                }
                Ok(()) => {
                    batch[index].state = CallState::Ready {
                        invocation,
                        context,
                    }
                }
                Err(error)
                    if error.kind() == ToolErrorKind::Cancelled
                        && control.cancellation.is_cancelled() =>
                {
                    batch[index].state = CallState::PreparationFailed(error);
                    return Ok(true);
                }
                Err(error) => finish_call(control, &mut batch[index], Err(error)).await?,
            }
        }
        NextEvent::Completed { index, result } => {
            if matches!(&result, Err(error) if error.kind() == ToolErrorKind::Cancelled)
                && control.cancellation.is_cancelled()
            {
                return Ok(true);
            }
            close_call_host_input(core, &batch[index].id, pending_input);
            finish_call(control, &mut batch[index], result).await?;
        }
        NextEvent::Worker(WorkerEvent::Progress { index, progress }) => {
            emit_progress_while_servicing_commands(
                core,
                control,
                pending_input,
                commands_open,
                RunEvent::ToolUpdated {
                    call_id: batch[index].id.clone(),
                    progress,
                },
            )
            .await?;
        }
        NextEvent::Worker(WorkerEvent::HostInput { index, request }) => {
            let request_id = request.request.id().clone();
            if let Some(owner) = observed_input.get(&request_id) {
                let message = format!(
                    "duplicate host input request ID from tool calls '{}' and '{}'",
                    owner.as_str(),
                    batch[index].id.as_str()
                );
                let _ = request.response.send(Err(Error::InvalidHostResponse {
                    message: message.clone(),
                }));
                return Err(Error::InvalidHostResponse { message });
            }
            observed_input.insert(request_id.clone(), batch[index].id.clone());
            core.set_state(SessionState::WaitingForHostInput);
            let event_request = request.request.clone();
            pending_input.insert(request_id, (batch[index].id.clone(), request));
            emit(
                control.events,
                control.cancellation,
                RunEvent::HostInputRequested {
                    call_id: batch[index].id.clone(),
                    request: event_request,
                },
            )
            .await?;
        }
        NextEvent::Command(Some(command)) => {
            handle_command(core, command, pending_input, control.steering)
        }
        NextEvent::Command(None) => {}
        NextEvent::Cancelled => return Ok(true),
    }
    Ok(false)
}

async fn emit_progress_while_servicing_commands(
    core: &Arc<SessionCore>,
    control: &mut RunControl<'_>,
    pending: &mut BTreeMap<HostInputId, (ToolCallId, HostInputEnvelope)>,
    commands_open: &mut bool,
    event: RunEvent,
) -> Result<(), Error> {
    let events = control.events;
    let cancellation = control.cancellation;
    let commands = &mut *control.commands;
    let steering = &mut *control.steering;
    let delivery = events.send(event);
    tokio::pin!(delivery);
    loop {
        tokio::select! {
            biased;
            command = commands.recv(), if *commands_open => match command {
                Some(command) => handle_command(core, command, pending, steering),
                None => *commands_open = false,
            },
            result = &mut delivery => {
                return result.map_err(|_| Error::Interrupted {
                    message: "run event consumer was dropped".into(),
                });
            }
            () = cancellation.cancelled() => return Err(Error::Cancelled),
        }
    }
}

fn close_call_host_input(
    core: &Arc<SessionCore>,
    call_id: &ToolCallId,
    pending: &mut BTreeMap<HostInputId, (ToolCallId, HostInputEnvelope)>,
) {
    let request_ids = pending
        .iter()
        .filter(|(_, (owner, _))| owner == call_id)
        .map(|(request_id, _)| request_id.clone())
        .collect::<Vec<_>>();
    for request_id in request_ids {
        if let Some((_, request)) = pending.remove(&request_id) {
            let _ = request.response.send(Err(Error::InvalidHostResponse {
                message: "tool completed before answering its host input request".into(),
            }));
        }
    }
    if pending.is_empty() {
        core.set_state(SessionState::Running);
    }
}

async fn finish_call(
    control: &mut RunControl<'_>,
    entry: &mut BatchCall<'_>,
    result: Result<ToolOutput, ToolError>,
) -> Result<(), Error> {
    let normalized = match &result {
        Ok(output) => ToolResult {
            id: entry.call.id.clone(),
            ok: true,
            content: output.content().to_owned(),
        },
        Err(error) => failed_result(&entry.call, error),
    };
    if let Some(started) = entry.execution_started {
        tracing::debug!(
            execution_duration_ms = started.elapsed().as_millis() as u64,
            "tool call execution completed"
        );
    }
    let completion = match result {
        Ok(output) => ToolCompletion::Success(output),
        Err(error) => {
            ToolCompletion::Failure(ToolFailure::new(error.kind(), error.message().to_owned()))
        }
    };
    entry.result = Some(normalized);
    entry.state = CallState::Resolved;
    emit(
        control.events,
        control.cancellation,
        RunEvent::ToolFinished {
            call_id: entry.id.clone(),
            result: completion,
        },
    )
    .await?;
    Ok(())
}

fn failed_result(call: &ToolCall, error: &ToolError) -> ToolResult {
    ToolResult {
        id: call.id.clone(),
        ok: false,
        content: error.message().to_owned(),
    }
}

fn handle_command(
    core: &Arc<SessionCore>,
    command: RunCommand,
    pending: &mut BTreeMap<HostInputId, (ToolCallId, HostInputEnvelope)>,
    steering: &mut crate::steering::SteeringQueue,
) {
    match command {
        RunCommand::Steer { input, accepted } => {
            let _ = accepted.send(steering.accept(input));
        }
        RunCommand::RetractSteering { id, completed } => {
            let _ = completed.send(steering.retract(&id));
        }
        RunCommand::Respond {
            request_id,
            response,
            accepted,
        } => {
            let Some((_, request)) = pending.get(&request_id) else {
                let _ = accepted.send(Err("host input request is not pending".into()));
                return;
            };
            if let Err(error) = request.request.validate(&response) {
                let _ = accepted.send(Err(error.to_string()));
                return;
            }
            let (_, request) = pending
                .remove(&request_id)
                .expect("pending request was checked");
            let delivered = request.response.send(Ok(response)).is_ok();
            let _ = accepted.send(if delivered {
                Ok(())
            } else {
                Err("host input requester was dropped".into())
            });
            if pending.is_empty() {
                core.set_state(SessionState::Running);
            }
        }
    }
}

fn append_results(batch: &mut [BatchCall<'_>], history: &mut Vec<Message>) {
    history.extend(batch.iter_mut().map(|entry| {
        Message::ToolResult(entry.result.take().expect("resolved call has a result"))
    }));
}

fn interrupt_batch(
    cancellation: &CancellationToken,
    batch: &mut [BatchCall<'_>],
    history: &mut Vec<Message>,
) {
    cancellation.cancel();
    let unresolved = batch
        .iter()
        .filter(|entry| !matches!(entry.state, CallState::Resolved))
        .count();
    tracing::debug!(unresolved_calls = unresolved, "tool batch cleanup started");
    for entry in batch.iter_mut() {
        if let CallState::Finishing(result) = &mut entry.state {
            let completed = result.take().and_then(|result| match result {
                Ok(output) => Some(ToolResult {
                    id: entry.call.id.clone(),
                    ok: true,
                    content: output.content().to_owned(),
                }),
                Err(error) if error.kind() == ToolErrorKind::Cancelled => None,
                Err(error) => Some(ToolResult {
                    id: entry.call.id.clone(),
                    ok: false,
                    content: error.message().to_owned(),
                }),
            });
            if let Some(result) = completed {
                entry.result = Some(result);
                entry.state = CallState::Resolved;
            }
        }
        if !matches!(entry.state, CallState::Resolved) {
            entry.result = Some(ToolResult {
                id: entry.call.id.clone(),
                ok: false,
                content: INTERRUPTED_TOOL_RESULT_CONTENT.into(),
            });
            entry.state = CallState::Resolved;
        }
    }
    append_results(batch, history);
}

async fn settle_running_after_cancellation(
    cancellation: &CancellationToken,
    batch: &mut [BatchCall<'_>],
    worker_rx: &mut mpsc::Receiver<WorkerEvent>,
) {
    cancellation.cancel();
    while batch
        .iter()
        .any(|entry| matches!(entry.state, CallState::Running(_)))
    {
        std::future::poll_fn(|cx| {
            let mut completed = false;
            for entry in batch.iter_mut() {
                let CallState::Running(future) = &mut entry.state else {
                    continue;
                };
                if let Poll::Ready(result) = future.as_mut().poll(cx) {
                    entry.state = CallState::Finishing(Some(result));
                    completed = true;
                }
            }
            if completed || worker_rx.poll_recv(cx).is_ready() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

async fn cancel_batch(
    error: Error,
    cancellation: &CancellationToken,
    batch: &mut [BatchCall<'_>],
    worker_rx: &mut mpsc::Receiver<WorkerEvent>,
    history: &mut Vec<Message>,
) -> Result<bool, Error> {
    settle_running_after_cancellation(cancellation, batch, worker_rx).await;
    interrupt_batch(cancellation, batch, history);
    if matches!(error, Error::Cancelled) {
        Ok(true)
    } else {
        Err(error)
    }
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
