use std::{collections::VecDeque, future::poll_fn, num::NonZeroUsize, sync::Arc, task::Poll};

use crate::{
    event::{ToolCompletion, ToolFailure},
    model::{Message, ToolCall, ToolResult},
    run::RunCommand,
    session::{SessionCore, SessionState},
    tool::{
        tool_progress_channel, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation,
        ToolInvocationSource, ToolOutput, ToolProgress,
    },
    Error, RunEvent, ToolCallId,
};

use super::{emit, emit_failure, Rho, RunControl, TOOL_PROGRESS_CAPACITY};

pub(super) const INTERRUPTED_TOOL_RESULT_CONTENT: &str = "tool call interrupted before completion";

#[derive(Clone)]
pub(super) struct PendingToolCall {
    pub(super) call: ToolCall,
    id: ToolCallId,
    source: ToolInvocationSource,
}

pub(super) struct StagedToolTurn {
    unresolved: VecDeque<PendingToolCall>,
}

impl StagedToolTurn {
    pub(super) fn model_requested(calls: Vec<ToolCall>) -> Self {
        Self::from_calls(calls, ToolInvocationSource::Model)
    }

    pub(super) fn host_requested(call: ToolCall) -> Self {
        Self::from_calls(vec![call], ToolInvocationSource::Host)
    }

    fn from_calls(calls: Vec<ToolCall>, source: ToolInvocationSource) -> Self {
        let unresolved = calls
            .into_iter()
            .map(|call| PendingToolCall {
                id: ToolCallId::from_string(call.id.clone())
                    .expect("validated provider tool call ID is nonempty"),
                call,
                source,
            })
            .collect();
        Self { unresolved }
    }

    pub(super) fn current(&self) -> Option<&PendingToolCall> {
        self.unresolved.front()
    }

    pub(super) fn resolve_current(&mut self, result: ToolResult, history: &mut Vec<Message>) {
        let pending = self
            .unresolved
            .pop_front()
            .expect("a resolved tool call must be pending");
        debug_assert_eq!(pending.id.as_str(), result.id);
        history.push(Message::ToolResult(result));
    }

    pub(super) fn interrupt_remaining(&mut self, history: &mut Vec<Message>) {
        history.extend(self.unresolved.drain(..).map(|pending| {
            Message::ToolResult(ToolResult {
                id: pending.id.into_string(),
                ok: false,
                content: INTERRUPTED_TOOL_RESULT_CONTENT.into(),
            })
        }));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolTurnStatus {
    Completed,
    Cancelled,
}

impl ToolTurnStatus {
    pub(super) fn is_cancelled(self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

pub(super) async fn execute_staged_tool_turn(
    core: &Arc<SessionCore>,
    runtime: &Rho,
    tool_turn: &mut StagedToolTurn,
    history: &mut Vec<Message>,
    control: &mut RunControl<'_>,
) -> Result<ToolTurnStatus, Error> {
    while let Some(pending) = tool_turn.current() {
        match emit(
            control.events,
            control.cancellation,
            RunEvent::ToolProposed {
                call: pending.call.clone(),
            },
        )
        .await
        {
            Ok(()) => {}
            Err(Error::Cancelled) => {
                tool_turn.interrupt_remaining(history);
                return Ok(ToolTurnStatus::Cancelled);
            }
            Err(error) => return Err(error),
        }
        match execute_tool(core, runtime, pending, control).await {
            Ok(result) => tool_turn.resolve_current(result, history),
            Err(failure) if matches!(&failure.error, Error::Cancelled) => {
                if let Some(result) = failure.completed_result {
                    tool_turn.resolve_current(result, history);
                }
                tool_turn.interrupt_remaining(history);
                return Ok(ToolTurnStatus::Cancelled);
            }
            Err(failure) => {
                core.set_state(SessionState::Failed);
                emit_failure(control.events, &failure.error).await;
                return Err(failure.error);
            }
        }
    }
    Ok(ToolTurnStatus::Completed)
}

pub(super) struct ToolExecutionFailure {
    pub(super) error: Error,
    pub(super) completed_result: Option<ToolResult>,
}

impl ToolExecutionFailure {
    fn before_completion(error: Error) -> Self {
        Self {
            error,
            completed_result: None,
        }
    }

    fn after_completion(error: Error, result: &ToolResult) -> Self {
        Self {
            error,
            completed_result: Some(result.clone()),
        }
    }
}

enum ToolLoopEvent {
    Completed(Result<ToolOutput, ToolError>),
    Cancelled,
    Progress(Option<ToolProgress>),
    HostInput(Option<crate::host_input::HostInputEnvelope>),
    Command(Option<RunCommand>),
}

async fn delivery_failure(
    error: Error,
    call: &ToolCall,
    future: &mut ToolFuture<'_>,
) -> ToolExecutionFailure {
    if matches!(&error, Error::Cancelled) {
        let completed = poll_fn(|context| {
            Poll::Ready(match future.as_mut().poll(context) {
                Poll::Ready(result) => Some(result),
                Poll::Pending => None,
            })
        })
        .await;
        if let Some(result) = completed {
            if matches!(&result, Err(error) if error.kind() == ToolErrorKind::Cancelled) {
                return ToolExecutionFailure::before_completion(Error::Cancelled);
            }
            let result = tool_result(call, &result);
            return ToolExecutionFailure::after_completion(Error::Cancelled, &result);
        }
    }
    ToolExecutionFailure::before_completion(error)
}

fn tool_result(call: &ToolCall, result: &Result<ToolOutput, ToolError>) -> ToolResult {
    match result {
        Ok(output) => ToolResult {
            id: call.id.clone(),
            ok: true,
            content: output.content().to_owned(),
        },
        Err(error) => ToolResult {
            id: call.id.clone(),
            ok: false,
            content: error.message().to_owned(),
        },
    }
}

pub(super) async fn execute_tool(
    core: &Arc<SessionCore>,
    runtime: &Rho,
    pending: &PendingToolCall,
    control: &mut RunControl<'_>,
) -> Result<ToolResult, ToolExecutionFailure> {
    let cancellation = control.cancellation;
    let events = control.events;
    let call = &pending.call;
    let call_id = &pending.id;
    let Some(tool) = runtime.tools.get(&call.name) else {
        let result = ToolResult {
            id: call.id.clone(),
            ok: false,
            content: format!("tool '{}' is unavailable", call.name),
        };
        emit(
            events,
            cancellation,
            RunEvent::ToolFinished {
                call_id: call_id.clone(),
                result: ToolCompletion::Unavailable,
            },
        )
        .await
        .map_err(|error| ToolExecutionFailure::after_completion(error, &result))?;
        return Ok(result);
    };

    emit(
        events,
        cancellation,
        RunEvent::ToolStarted {
            call_id: call_id.clone(),
            name: call.name.clone(),
            metadata: tool.start_metadata(&call.arguments),
        },
    )
    .await
    .map_err(ToolExecutionFailure::before_completion)?;
    let (progress, mut progress_receiver) =
        tool_progress_channel(NonZeroUsize::new(TOOL_PROGRESS_CAPACITY).unwrap());
    let invocation = match pending.source {
        ToolInvocationSource::Model => ToolInvocation::new(call_id.clone(), call.arguments.clone()),
        ToolInvocationSource::Host => {
            ToolInvocation::from_host(call_id.clone(), call.arguments.clone())
        }
    };
    let (host_input, mut host_input_receiver) =
        crate::host_input::channel(TOOL_PROGRESS_CAPACITY, cancellation.clone());
    let context = ToolContext::with_security(
        runtime.workspace.clone(),
        Arc::clone(&runtime.workspace_policy),
        Arc::clone(&runtime.approval_handler),
        core.approvals(),
        Arc::clone(&runtime.approval_audit),
        cancellation.clone(),
        progress,
    )
    .with_host_input(host_input);
    let mut future = tool.call(invocation, context);
    let mut pending_input = std::collections::BTreeMap::new();
    let mut progress_open = true;
    let mut host_input_open = true;
    let mut commands_open = true;
    let result = loop {
        let event = tokio::select! {
            biased;
            result = &mut future => ToolLoopEvent::Completed(result),
            () = cancellation.cancelled() => ToolLoopEvent::Cancelled,
            progress = progress_receiver.recv(), if progress_open => {
                ToolLoopEvent::Progress(progress)
            }
            request = host_input_receiver.recv(), if host_input_open => {
                ToolLoopEvent::HostInput(request)
            }
            command = control.commands.recv(), if commands_open => {
                ToolLoopEvent::Command(command)
            }
        };
        match event {
            ToolLoopEvent::Completed(result) => break result,
            ToolLoopEvent::Cancelled => {
                return Err(ToolExecutionFailure::before_completion(Error::Cancelled));
            }
            ToolLoopEvent::Progress(Some(progress)) => {
                if let Err(error) = emit(
                    events,
                    cancellation,
                    RunEvent::ToolUpdated {
                        call_id: call_id.clone(),
                        progress,
                    },
                )
                .await
                {
                    return Err(delivery_failure(error, call, &mut future).await);
                }
            }
            ToolLoopEvent::Progress(None) => progress_open = false,
            ToolLoopEvent::HostInput(Some(request)) => {
                let request_id = request.request.id().clone();
                match pending_input.entry(request_id) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        core.set_state(SessionState::WaitingForHostInput);
                        let event_request = request.request.clone();
                        entry.insert(request);
                        if let Err(error) = emit(
                            events,
                            cancellation,
                            RunEvent::HostInputRequested {
                                request: event_request,
                            },
                        )
                        .await
                        {
                            return Err(delivery_failure(error, call, &mut future).await);
                        }
                    }
                    std::collections::btree_map::Entry::Occupied(_) => {
                        let _ = request.response.send(Err(Error::InvalidHostResponse {
                            message: "duplicate host input request ID".into(),
                        }));
                    }
                }
            }
            ToolLoopEvent::HostInput(None) => host_input_open = false,
            ToolLoopEvent::Command(Some(command)) => {
                handle_tool_command(core, command, &mut pending_input, control.steering);
            }
            ToolLoopEvent::Command(None) => commands_open = false,
        }
    };
    if matches!(&result, Err(error) if error.kind() == ToolErrorKind::Cancelled)
        && cancellation.is_cancelled()
    {
        return Err(ToolExecutionFailure::before_completion(Error::Cancelled));
    }
    let normalized_result = tool_result(call, &result);
    let completion = match result {
        Ok(output) => ToolCompletion::Success(output),
        Err(error) => {
            ToolCompletion::Failure(ToolFailure::new(error.kind(), error.message().to_owned()))
        }
    };
    core.set_state(SessionState::Running);
    while let Some(progress) = progress_receiver.try_recv() {
        emit(
            events,
            cancellation,
            RunEvent::ToolUpdated {
                call_id: call_id.clone(),
                progress,
            },
        )
        .await
        .map_err(|error| ToolExecutionFailure::after_completion(error, &normalized_result))?;
    }
    emit(
        events,
        cancellation,
        RunEvent::ToolFinished {
            call_id: call_id.clone(),
            result: completion,
        },
    )
    .await
    .map_err(|error| ToolExecutionFailure::after_completion(error, &normalized_result))?;
    Ok(normalized_result)
}

fn handle_tool_command(
    core: &Arc<SessionCore>,
    command: RunCommand,
    pending: &mut std::collections::BTreeMap<
        crate::HostInputId,
        crate::host_input::HostInputEnvelope,
    >,
    steering: &mut crate::steering::SteeringQueue,
) {
    match command {
        RunCommand::Steer { input, accepted } => {
            let id = steering.accept(input);
            let _ = accepted.send(id);
        }
        RunCommand::RetractSteering { id, completed } => {
            let _ = completed.send(steering.retract(&id));
        }
        RunCommand::Respond {
            request_id,
            response,
            accepted,
        } => {
            let Some(request) = pending.get(&request_id) else {
                let _ = accepted.send(Err("host input request is not pending".into()));
                return;
            };
            if let Err(error) = request.request.validate(&response) {
                let _ = accepted.send(Err(error.to_string()));
                return;
            }
            let request = pending
                .remove(&request_id)
                .expect("pending request was checked above");
            let delivered = request.response.send(Ok(response)).is_ok();
            let _ = if delivered {
                accepted.send(Ok(()))
            } else {
                accepted.send(Err("host input requester was dropped".into()))
            };
            if pending.is_empty() {
                core.set_state(SessionState::Running);
            }
        }
    }
}
