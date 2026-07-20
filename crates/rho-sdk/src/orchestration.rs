use std::{num::NonZeroUsize, sync::Arc};

use tokio::sync::mpsc;

use crate::{
    client::Rho,
    event::{RunOutcome, StopReason},
    model::{
        AssistantMessage, ContentBlock, Message, ModelEvent, ModelRequest, ModelResponse,
        ModelUsage,
    },
    provider::{provider_event_channel, ModelProvider, ProviderCancellationMode},
    run::RunCommand,
    session::{HistoryMetrics, SessionCore, SessionState, UserInput},
    steering::SteeringQueue,
    CancellationToken, Error, ProviderError, ProviderErrorKind, Retryability, RunEvent, RunId,
};

const PROVIDER_EVENT_CAPACITY: usize = 16;
const TOOL_PROGRESS_CAPACITY: usize = 16;
const INVALID_RESPONSE_ATTEMPTS: usize = 2;
/// Maximum logical provider requests for one model turn, including malformed
/// responses and retryable failures.
const PROVIDER_TURN_ATTEMPTS: usize = 4;
/// Backoff before the first retryable-failure retry; doubles per retry.
const RETRYABLE_REQUEST_BASE_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

mod stream_capture;
mod tool_turn;

use stream_capture::{capture_provider_event, StreamCapture};
use tool_turn::{execute_tool, StagedToolTurn};

pub(crate) async fn execute_run(
    core: Arc<SessionCore>,
    runtime: Rho,
    run_id: RunId,
    input: UserInput,
    cancellation: CancellationToken,
    events: mpsc::Sender<RunEvent>,
    mut commands: mpsc::Receiver<RunCommand>,
) -> Result<RunOutcome, Error> {
    let (mut history, revision) = core.snapshot();
    history.push(Message::User(input.into_blocks()));
    match emit(
        &events,
        &cancellation,
        RunEvent::Started {
            run_id: run_id.clone(),
            revision,
        },
    )
    .await
    {
        Ok(()) => {}
        Err(Error::Cancelled) => {
            return commit_cancelled_history(core, history, &events).await;
        }
        Err(error) => return Err(error),
    }

    let mut accumulated_usage = ModelUsage::default();
    let mut steering = SteeringQueue::new();
    // The tool set is immutable for the duration of a run, so build the specs
    // (which deep-clone every tool's JSON schema) once instead of per step.
    let tool_specs = runtime.tools.specs();
    for step in 1..=runtime.max_steps.get() {
        drain_commands(&mut commands, &mut steering);
        match apply_staged_steering(&mut steering, &mut history, &events, &cancellation).await {
            Ok(()) => {}
            Err(Error::Cancelled) => {
                return commit_cancelled_history(core, history, &events).await;
            }
            Err(error) => return Err(error),
        }
        let request_scope = ProviderRequestScope {
            runtime: &runtime,
            session_id: core.id(),
            run_id: &run_id,
            step_index: step,
        };
        match maybe_compact(
            &core,
            request_scope,
            &tool_specs,
            &mut history,
            &cancellation,
            &events,
        )
        .await
        {
            Ok(()) => {}
            Err(Error::Cancelled) => {
                return commit_cancelled_history(core, history, &events).await;
            }
            Err(error) => {
                core.set_state(SessionState::Failed);
                emit_failure(&events, &error).await;
                return Err(error);
            }
        }
        match emit(&events, &cancellation, RunEvent::StepStarted { step }).await {
            Ok(()) => {}
            Err(Error::Cancelled) => {
                return commit_cancelled_history(core, history, &events).await;
            }
            Err(error) => return Err(error),
        }

        let mut control = RunControl {
            cancellation: &cancellation,
            events: &events,
            commands: &mut commands,
            steering: &mut steering,
        };
        let (response, mut capture) = match request_valid_response(
            request_scope,
            &history,
            &tool_specs,
            &accumulated_usage,
            runtime.reasoning_level,
            core.prompt_cache_key().as_deref(),
            &mut control,
        )
        .await
        {
            Ok(result) => result,
            Err(error) if cancellation.is_cancelled() => {
                return commit_cancellation(core, history, error.capture, &events).await;
            }
            Err(error) => {
                let sdk_error = Error::from(error.error);
                core.set_state(SessionState::Failed);
                emit_failure(&events, &sdk_error).await;
                return Err(sdk_error);
            }
        };
        accumulated_usage = accumulated_usage.saturating_add(capture.usage());

        let ModelResponse::Assistant(content) = response;
        let tool_calls = content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall(call) => Some(call.clone()),
                ContentBlock::Text(_) | ContentBlock::Image(_) => None,
            })
            .collect::<Vec<_>>();
        let (reasoning_summary, provider_context) = capture.take_assistant_context();
        let assistant = AssistantMessage {
            content,
            provenance: Some(runtime.provider.identity()),
            reasoning_summary,
            provider_context,
        };
        history.push(Message::assistant(assistant));
        drain_commands(control.commands, control.steering);
        let was_steered = control.steering.has_staged();

        if tool_calls.is_empty() && !was_steered {
            let content = final_assistant_content(&history);
            let revision = core.commit(history)?;
            let outcome =
                RunOutcome::new(content, accumulated_usage, StopReason::EndTurn, revision);
            core.set_state(SessionState::Completed);
            send_terminal(
                &events,
                RunEvent::Completed {
                    outcome: outcome.clone(),
                },
            )
            .await;
            return Ok(outcome);
        }

        let mut tool_turn = StagedToolTurn::new(tool_calls);
        while let Some(pending) = tool_turn.current() {
            match emit(
                &events,
                &cancellation,
                RunEvent::ToolProposed {
                    call: pending.call.clone(),
                },
            )
            .await
            {
                Ok(()) => {}
                Err(Error::Cancelled) => {
                    tool_turn.interrupt_remaining(&mut history);
                    return commit_cancelled_history(core, history, &events).await;
                }
                Err(error) => return Err(error),
            }
            match execute_tool(&core, &runtime, pending, &mut control).await {
                Ok(result) => tool_turn.resolve_current(result, &mut history),
                Err(failure) if matches!(&failure.error, Error::Cancelled) => {
                    if let Some(result) = failure.completed_result {
                        tool_turn.resolve_current(result, &mut history);
                    }
                    tool_turn.interrupt_remaining(&mut history);
                    return commit_cancelled_history(core, history, &events).await;
                }
                Err(failure) => {
                    core.set_state(SessionState::Failed);
                    emit_failure(&events, &failure.error).await;
                    return Err(failure.error);
                }
            }
        }
        match apply_staged_steering(
            control.steering,
            &mut history,
            control.events,
            control.cancellation,
        )
        .await
        {
            Ok(()) => {}
            Err(Error::Cancelled) => {
                return commit_cancelled_history(core, history, &events).await;
            }
            Err(error) => return Err(error),
        }
    }

    let last_content = final_assistant_content(&history);
    let revision = core.commit(history)?;
    let outcome = RunOutcome::new(
        last_content,
        accumulated_usage,
        StopReason::MaxSteps,
        revision,
    );
    core.set_state(SessionState::Completed);
    send_terminal(
        &events,
        RunEvent::Completed {
            outcome: outcome.clone(),
        },
    )
    .await;
    Ok(outcome)
}

/// Content of the newest completed assistant message, cloned once for the
/// terminal run outcome instead of re-cloned on every step.
fn final_assistant_content(history: &[Message]) -> Vec<ContentBlock> {
    history
        .iter()
        .rev()
        .find_map(Message::completed_assistant_content)
        .map(<[ContentBlock]>::to_vec)
        .unwrap_or_default()
}

async fn maybe_compact(
    core: &Arc<SessionCore>,
    scope: ProviderRequestScope<'_>,
    tool_specs: &[crate::model::ToolSpec],
    history: &mut Vec<Message>,
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<(), Error> {
    let Some(policy) = &scope.runtime.compaction_policy else {
        return Ok(());
    };
    let context_tokens = crate::model::context::estimate_context_tokens(history, tool_specs);
    if !policy.should_compact(history.len(), context_tokens) {
        return Ok(());
    }
    let compactor = scope
        .runtime
        .compactor
        .as_ref()
        .expect("builder requires a compactor for automatic policy");
    emit(
        events,
        cancellation,
        RunEvent::CompactionStarted {
            trigger: crate::CompactionTrigger::Automatic,
            message_count: history.len(),
        },
    )
    .await?;
    let previous = HistoryMetrics::from_history(history);
    let request = crate::CompactionRequest::new(history.clone(), cancellation.clone())
        .with_request_context(
            scope.session_id.clone(),
            scope.runtime.usage_parent_session_id.clone(),
            scope.run_id.clone(),
            Some(scope.step_index),
            scope
                .runtime
                .workspace
                .as_ref()
                .map(|workspace| workspace.root().to_path_buf()),
        );
    let output = match compactor.cancellation_mode() {
        crate::CompactorCancellationMode::Cooperative => compactor.compact(request).await?,
        crate::CompactorCancellationMode::External => {
            tokio::select! {
                result = compactor.compact(request) => result?,
                () = cancellation.cancelled() => return Err(Error::Cancelled),
            }
        }
    };
    let (replacement, usage) = output.into_parts();
    let outcome = core.commit_compaction(previous, replacement.clone(), usage)?;
    *history = replacement;
    emit(
        events,
        cancellation,
        RunEvent::CompactionCompleted {
            trigger: crate::CompactionTrigger::Automatic,
            outcome,
        },
    )
    .await
}

struct RequestFailure {
    error: ProviderError,
    capture: StreamCapture,
}

struct RunControl<'a> {
    cancellation: &'a CancellationToken,
    events: &'a mpsc::Sender<RunEvent>,
    commands: &'a mut mpsc::Receiver<RunCommand>,
    steering: &'a mut SteeringQueue,
}

#[derive(Clone, Copy)]
struct ProviderRequestScope<'a> {
    runtime: &'a Rho,
    session_id: &'a crate::SessionId,
    run_id: &'a RunId,
    step_index: usize,
}

async fn request_valid_response(
    scope: ProviderRequestScope<'_>,
    history: &[Message],
    tools: &[crate::model::ToolSpec],
    accumulated_usage: &ModelUsage,
    reasoning_level: crate::ReasoningLevel,
    prompt_cache_key: Option<&str>,
    control: &mut RunControl<'_>,
) -> Result<(ModelResponse, StreamCapture), RequestFailure> {
    let mut next_attempt_index = 1;
    let mut provider_turn_attempts = 0;
    let mut invalid_responses = 0;
    let mut failed_requests = 0;
    loop {
        provider_turn_attempts += 1;
        let result = provider_turn(
            scope.runtime.provider.as_ref(),
            history,
            tools,
            accumulated_usage,
            reasoning_level,
            prompt_cache_key,
            control,
        )
        .await;
        let (response, capture) = match result {
            Ok((response, mut capture)) => {
                next_attempt_index =
                    record_failed_provider_attempts(&scope, next_attempt_index, &mut capture).await;
                let outcome = if valid_response(&response) {
                    crate::ProviderRequestOutcome::Completed
                } else {
                    crate::ProviderRequestOutcome::InvalidResponse
                };
                record_request_usage(&scope, next_attempt_index, capture.usage().clone(), outcome)
                    .await;
                next_attempt_index += 1;
                (response, capture)
            }
            Err(mut failure) => {
                next_attempt_index = record_failed_provider_attempts(
                    &scope,
                    next_attempt_index,
                    &mut failure.capture,
                )
                .await;
                let outcome = if control.cancellation.is_cancelled() {
                    crate::ProviderRequestOutcome::Cancelled
                } else {
                    crate::ProviderRequestOutcome::Failed(failure.error.kind())
                };
                record_request_usage(
                    &scope,
                    next_attempt_index,
                    failure.capture.usage().clone(),
                    outcome,
                )
                .await;
                next_attempt_index += 1;
                failed_requests += 1;
                if control.cancellation.is_cancelled()
                    || !failure.error.is_retryable()
                    || provider_turn_attempts >= PROVIDER_TURN_ATTEMPTS
                {
                    return Err(failure);
                }
                let detail = format!(
                    "retrying after provider attempt {provider_turn_attempts} of {PROVIDER_TURN_ATTEMPTS}: {}",
                    failure.error.message()
                );
                let _ = emit(
                    control.events,
                    control.cancellation,
                    RunEvent::ProviderStreamReset {
                        reason: crate::ProviderStreamResetReason::RetryableFailure(
                            failure.error.kind(),
                        ),
                        detail,
                    },
                )
                .await;
                let delay = RETRYABLE_REQUEST_BASE_DELAY * 2u32.pow(failed_requests as u32 - 1);
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = control.cancellation.cancelled() => return Err(failure),
                }
                continue;
            }
        };
        if valid_response(&response) {
            return Ok((response, capture));
        }
        invalid_responses += 1;
        if invalid_responses >= INVALID_RESPONSE_ATTEMPTS
            || provider_turn_attempts >= PROVIDER_TURN_ATTEMPTS
        {
            return Err(RequestFailure {
                error: ProviderError::new(
                    ProviderErrorKind::InvalidResponse,
                    "provider returned an empty assistant response",
                    Retryability::Permanent,
                ),
                capture,
            });
        }
        let detail = format!(
            "retrying malformed provider response after provider attempt {provider_turn_attempts} of {PROVIDER_TURN_ATTEMPTS}"
        );
        // Preserve the 1.0 activity event while typed reset consumers migrate.
        let _ = emit(
            control.events,
            control.cancellation,
            RunEvent::ProviderActivity {
                kind: crate::PROVIDER_ACTIVITY_INVALID_RESPONSE_RETRY.into(),
                detail: detail.clone(),
            },
        )
        .await;
        let _ = emit(
            control.events,
            control.cancellation,
            RunEvent::ProviderStreamReset {
                reason: crate::ProviderStreamResetReason::InvalidResponse,
                detail,
            },
        )
        .await;
    }
}

async fn record_failed_provider_attempts(
    scope: &ProviderRequestScope<'_>,
    mut next_attempt_index: usize,
    capture: &mut StreamCapture,
) -> usize {
    for (kind, usage) in capture.take_failed_attempts() {
        record_request_usage(
            scope,
            next_attempt_index,
            usage,
            crate::ProviderRequestOutcome::Failed(kind),
        )
        .await;
        next_attempt_index += 1;
    }
    next_attempt_index
}

async fn record_request_usage(
    scope: &ProviderRequestScope<'_>,
    attempt_index: usize,
    usage: ModelUsage,
    outcome: crate::ProviderRequestOutcome,
) {
    let mut context = crate::ProviderRequestUsageContext::new(
        scope.runtime.provider.identity(),
        scope.session_id.clone(),
        scope.run_id.clone(),
        scope.step_index,
        attempt_index,
        scope
            .runtime
            .workspace
            .as_ref()
            .map(|workspace| workspace.root().to_path_buf()),
        scope.runtime.usage_purpose.clone(),
    );
    if let Some(parent_session_id) = &scope.runtime.usage_parent_session_id {
        context = context.with_parent_session_id(parent_session_id.clone());
    }
    scope
        .runtime
        .usage_recording
        .record(crate::ProviderRequestUsageEvent::observed(
            context, usage, outcome,
        ))
        .await;
}

fn valid_response(response: &ModelResponse) -> bool {
    let ModelResponse::Assistant(content) = response;
    !content.is_empty()
        && content.iter().all(|block| match block {
            ContentBlock::ToolCall(call) => {
                !call.id.is_empty() && !call.name.is_empty() && call.arguments.is_object()
            }
            ContentBlock::Text(_) | ContentBlock::Image(_) => true,
        })
}

async fn provider_turn(
    provider: &dyn ModelProvider,
    history: &[Message],
    tools: &[crate::model::ToolSpec],
    accumulated_usage: &ModelUsage,
    reasoning_level: crate::ReasoningLevel,
    prompt_cache_key: Option<&str>,
    control: &mut RunControl<'_>,
) -> Result<(ModelResponse, StreamCapture), RequestFailure> {
    let (provider_events, mut receiver) =
        provider_event_channel(NonZeroUsize::new(PROVIDER_EVENT_CAPACITY).unwrap());
    let request = ModelRequest {
        messages: history,
        tools,
        cancellation: control.cancellation.clone(),
        reasoning_level,
        prompt_cache_key,
    };
    let cancellation_mode = provider.cancellation_mode();
    let mut future = provider.send_turn_stream(request, provider_events);
    let identity = provider.identity();
    let mut capture = StreamCapture::default();
    let mut stream_open = true;
    let mut commands_open = true;
    let result = loop {
        tokio::select! {
            result = &mut future => break result,
            event = receiver.recv_stream_event(), if stream_open => {
                match event {
                    Some(crate::provider::ProviderStreamEvent::Model(event)) => {
                        if let Err(error) = handle_provider_event(
                            event,
                            &identity,
                            accumulated_usage,
                            &mut capture,
                            control.events,
                            control.cancellation,
                        ).await {
                            if control.cancellation.is_cancelled() {
                                drop(future);
                                drain_cancelled_provider_events(
                                    &mut receiver,
                                    &identity,
                                    &mut capture,
                                );
                            }
                            return Err(RequestFailure { error, capture });
                        }
                    }
                    Some(crate::provider::ProviderStreamEvent::Request(event)) => {
                        if let Err(error) = handle_provider_request_event(
                            event,
                            &mut capture,
                            control.events,
                            control.cancellation,
                        ).await {
                            return Err(RequestFailure { error, capture });
                        }
                    }
                    None => stream_open = false,
                }
            }
            command = control.commands.recv(), if commands_open => {
                match command {
                    Some(command) => accept_non_tool_command(command, control.steering),
                    None => commands_open = false,
                }
            }
            () = control.cancellation.cancelled() => {
                if cancellation_mode == ProviderCancellationMode::Cooperative {
                    drain_cooperative_provider_on_cancellation(
                        &mut future,
                        &mut receiver,
                        &identity,
                        &mut capture,
                    )
                    .await;
                }
                drop(future);
                drain_cancelled_provider_events(&mut receiver, &identity, &mut capture);
                return Err(RequestFailure {
                    error: ProviderError::interrupted("provider request cancelled"),
                    capture,
                });
            }
        }
    };
    while let Some(event) = receiver.try_recv_stream_event() {
        let result = match event {
            crate::provider::ProviderStreamEvent::Model(event) => {
                handle_provider_event(
                    event,
                    &identity,
                    accumulated_usage,
                    &mut capture,
                    control.events,
                    control.cancellation,
                )
                .await
            }
            crate::provider::ProviderStreamEvent::Request(event) => {
                handle_provider_request_event(
                    event,
                    &mut capture,
                    control.events,
                    control.cancellation,
                )
                .await
            }
        };
        if let Err(error) = result {
            if control.cancellation.is_cancelled() {
                drain_cancelled_provider_events(&mut receiver, &identity, &mut capture);
            }
            return Err(RequestFailure { error, capture });
        }
    }
    match result {
        Ok(response) => Ok((response, capture)),
        Err(error) => Err(RequestFailure { error, capture }),
    }
}

async fn apply_staged_steering(
    steering: &mut SteeringQueue,
    history: &mut Vec<Message>,
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
) -> Result<(), Error> {
    let ids = steering.staged_ids();
    if ids.is_empty() {
        return Ok(());
    }
    // Publish before mutating history so cancellation cannot hide applied IDs from hosts.
    // There is deliberately no await between successful publication and the mutation.
    emit(events, cancellation, RunEvent::SteeringApplied { ids }).await?;
    steering.apply(history);
    Ok(())
}

fn accept_non_tool_command(command: RunCommand, steering: &mut SteeringQueue) {
    match command {
        RunCommand::Steer { input, accepted } => {
            let id = steering.accept(input);
            let _ = accepted.send(id);
        }
        RunCommand::RetractSteering { id, completed } => {
            let _ = completed.send(steering.retract(&id));
        }
        RunCommand::Respond { accepted, .. } => {
            let _ = accepted.send(Err("no host input request is awaiting a response".into()));
        }
    }
}

fn drain_commands(commands: &mut mpsc::Receiver<RunCommand>, steering: &mut SteeringQueue) {
    while let Ok(command) = commands.try_recv() {
        accept_non_tool_command(command, steering);
    }
}

async fn handle_provider_request_event(
    event: crate::provider::ProviderRequestEvent,
    capture: &mut StreamCapture,
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
) -> Result<(), ProviderError> {
    let crate::provider::ProviderRequestEvent::RequestAttemptFailed { kind, usage } = event;
    capture.record_request_attempt_failure(kind, usage);
    emit(
        events,
        cancellation,
        RunEvent::ProviderActivity {
            kind: crate::PROVIDER_ACTIVITY_REQUEST_RETRY.into(),
            detail: "retrying after a failed physical provider request".into(),
        },
    )
    .await
    .map_err(|error| ProviderError::interrupted(error.to_string()))
}

async fn handle_provider_event(
    event: ModelEvent,
    identity: &crate::model::ModelIdentity,
    accumulated_usage: &ModelUsage,
    capture: &mut StreamCapture,
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
) -> Result<(), ProviderError> {
    let run_event = capture_provider_event(event, identity, accumulated_usage, capture);
    emit(events, cancellation, run_event)
        .await
        .map_err(|error| ProviderError::interrupted(error.to_string()))
}

async fn drain_cooperative_provider_on_cancellation(
    future: &mut crate::provider::ProviderFuture<'_>,
    receiver: &mut crate::provider::ProviderEventReceiver,
    identity: &crate::model::ModelIdentity,
    capture: &mut StreamCapture,
) {
    let mut stream_open = true;
    loop {
        tokio::select! {
            biased;
            event = receiver.recv_stream_event(), if stream_open => {
                match event {
                    Some(crate::provider::ProviderStreamEvent::Model(event)) => {
                        let _ = capture_provider_event(
                            event,
                            identity,
                            &ModelUsage::default(),
                            capture,
                        );
                    }
                    Some(crate::provider::ProviderStreamEvent::Request(
                        crate::provider::ProviderRequestEvent::RequestAttemptFailed { kind, usage }
                    )) => {
                        capture.record_request_attempt_failure(kind, usage);
                    }
                    None => stream_open = false,
                }
            }
            _ = &mut *future => break,
        }
    }
}

fn drain_cancelled_provider_events(
    receiver: &mut crate::provider::ProviderEventReceiver,
    identity: &crate::model::ModelIdentity,
    capture: &mut StreamCapture,
) {
    while let Some(event) = receiver.try_recv_stream_event() {
        match event {
            crate::provider::ProviderStreamEvent::Model(event) => {
                // Cancellation-sensitive host publication must not prevent capture of
                // events the provider had already queued before its future was dropped.
                let _ = capture_provider_event(event, identity, &ModelUsage::default(), capture);
            }
            crate::provider::ProviderStreamEvent::Request(
                crate::provider::ProviderRequestEvent::RequestAttemptFailed { kind, usage },
            ) => {
                capture.record_request_attempt_failure(kind, usage);
            }
        }
    }
}

async fn commit_cancellation(
    core: Arc<SessionCore>,
    mut history: Vec<Message>,
    capture: StreamCapture,
    events: &mpsc::Sender<RunEvent>,
) -> Result<RunOutcome, Error> {
    if let Some(aborted) = capture.into_aborted_assistant() {
        history.push(Message::AbortedAssistant(Box::new(aborted)));
    }
    commit_cancelled_history(core, history, events).await
}

async fn commit_cancelled_history(
    core: Arc<SessionCore>,
    history: Vec<Message>,
    events: &mpsc::Sender<RunEvent>,
) -> Result<RunOutcome, Error> {
    let revision = core.commit(history)?;
    core.set_state(SessionState::Cancelling);
    send_terminal(events, RunEvent::Cancelled { revision }).await;
    Err(Error::Cancelled)
}

async fn emit(
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
    event: RunEvent,
) -> Result<(), Error> {
    tokio::select! {
        biased;
        result = events.send(event) => result.map_err(|_| Error::Interrupted {
            message: "run event consumer was dropped".into(),
        }),
        () = cancellation.cancelled() => Err(Error::Cancelled),
    }
}

async fn send_terminal(events: &mpsc::Sender<RunEvent>, event: RunEvent) {
    let _ = events.send(event).await;
}

async fn emit_failure(events: &mpsc::Sender<RunEvent>, error: &Error) {
    let diagnostic = match error {
        Error::Provider(error) => error.diagnostic(),
        _ => None,
    };
    if let Some(detail) = diagnostic {
        send_terminal(
            events,
            RunEvent::ProviderDiagnostic {
                detail: crate::ProviderDiagnostic::new(detail),
            },
        )
        .await;
    }
    send_terminal(
        events,
        RunEvent::Failed {
            message: error.to_string(),
            retryability: if error.is_retryable() {
                Retryability::Retryable
            } else {
                Retryability::Permanent
            },
        },
    )
    .await;
}

#[cfg(test)]
#[path = "orchestration_tests.rs"]
mod tests;
