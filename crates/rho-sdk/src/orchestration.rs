use std::{num::NonZeroUsize, sync::Arc, time::Instant};

use tokio::sync::mpsc;

use crate::{
    client::Rho,
    event::{RunOutcome, StopReason},
    model::{
        AssistantMessage, ContentBlock, Message, ModelEvent, ModelRequest, ModelResponse,
        ModelUsage,
    },
    provider::{provider_event_channel, ModelRequestOptions, ProviderCancellationMode},
    run::RunCommand,
    session::{HistoryMetrics, RunStart, SessionCore, SessionState},
    steering::SteeringQueue,
    CancellationToken, Error, ModelCallProfile, ProviderError, ProviderErrorKind, Retryability,
    RunEvent, RunId,
};

const PROVIDER_EVENT_CAPACITY: usize = 16;
const INVALID_RESPONSE_ATTEMPTS: usize = 2;
/// Maximum logical provider requests for one model turn, including malformed
/// responses and retryable failures.
const PROVIDER_TURN_ATTEMPTS: usize = 4;
/// Backoff before the first retryable-failure retry; doubles per retry.
const RETRYABLE_REQUEST_BASE_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
/// Upper bound when honoring provider `Retry-After` during auto-retry.
///
/// Keeps interactive turns from blocking on multi-hour quota resets. The full
/// provider wait still appears on the reset event and final error.
const MAX_HONORED_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

mod model_call_timer;
mod provider_cancellation;
mod run_hooks;
mod stream_capture;
mod terminal;
mod tool_batch;
mod tool_turn;

use model_call_timer::ModelCallTimer;
use provider_cancellation::{
    drain_cancelled_provider_events, drain_cooperative_provider_on_cancellation,
};
use run_hooks::RunHooks;
use stream_capture::{capture_provider_event, StreamCapture};
use terminal::{commit_terminal, commit_terminal_history, send_terminal, TerminalKind};
use tool_turn::{execute_staged_tool_turn, StagedToolTurn, ToolTurnStatus};

/// Runs one turn loop and reports its terminal outcome to lifecycle hooks.
///
/// The hook dispatch lives here, around the whole loop, so every exit path
/// reports exactly once. `run_completed` and `run_failed` fire per run, which is
/// what "notify me when a run finishes" means inside a long interactive session.
pub(crate) async fn execute_run(
    core: Arc<SessionCore>,
    runtime: Rho,
    run_id: RunId,
    start: RunStart,
    cancellation: CancellationToken,
    events: mpsc::Sender<RunEvent>,
    commands: mpsc::Receiver<RunCommand>,
) -> Result<RunOutcome, Error> {
    let hooks = RunHooks::new(&runtime, core.id().clone(), run_id.clone());
    let result = execute_turn_loop(
        core,
        runtime,
        run_id,
        start,
        cancellation,
        events,
        commands,
        &hooks,
    )
    .await;
    // Cancellation is an ordinary user-controlled stop in schema v1, which
    // has no cancellation event. Do not misreport it as `run_failed`.
    if !matches!(result, Err(Error::Cancelled)) {
        hooks.run_finished(&result);
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn execute_turn_loop(
    core: Arc<SessionCore>,
    runtime: Rho,
    run_id: RunId,
    start: RunStart,
    cancellation: CancellationToken,
    events: mpsc::Sender<RunEvent>,
    mut commands: mpsc::Receiver<RunCommand>,
    hooks: &RunHooks,
) -> Result<RunOutcome, Error> {
    let (mut history, revision) = core.snapshot();
    history.push(Message::User(start.input.into_blocks()));
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
            return commit_terminal_history(core, history, TerminalKind::Cancelled, &events).await;
        }
        Err(error) => return Err(error),
    }

    let mut accumulated_usage = ModelUsage::default();
    let mut steering = SteeringQueue::new();
    if let Some(call) = start.initial_tool_call {
        history.push(Message::Assistant(vec![ContentBlock::ToolCall(
            call.clone(),
        )]));
        let mut tool_turn = StagedToolTurn::host_requested(call);
        let mut control = RunControl {
            hooks,
            cancellation: &cancellation,
            events: &events,
            commands: &mut commands,
            steering: &mut steering,
        };
        let host_tool_result =
            run_staged_tool_turn(&core, &runtime, &mut tool_turn, &mut history, &mut control).await;
        history =
            match resolve_tool_turn_result(Arc::clone(&core), history, host_tool_result, &events)
                .await
            {
                Ok(history) => history,
                Err(terminal) => return *terminal,
            };
    }
    // The tool set is immutable for the duration of a run, so build the specs
    // (which deep-clone every tool's JSON schema) once instead of per step.
    let tool_specs = runtime.tools.specs();
    for step in 1..=runtime.max_steps.get() {
        drain_commands(&mut commands, &mut steering);
        match apply_staged_steering(&mut steering, &mut history, &events, &cancellation).await {
            Ok(()) => {}
            Err(Error::Cancelled) => {
                return commit_terminal_history(core, history, TerminalKind::Cancelled, &events)
                    .await;
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
                return commit_terminal_history(core, history, TerminalKind::Cancelled, &events)
                    .await;
            }
            Err(error @ Error::Interrupted { .. }) => return Err(error),
            Err(error) => {
                return commit_terminal(
                    core,
                    history,
                    StreamCapture::default(),
                    TerminalKind::Failed(error),
                    &events,
                )
                .await;
            }
        }
        // Emit before the provider call so quiet hosts still show context fill
        // while thinking and tool-call JSON stream (usage often arrives only at
        // the end of the OpenAI-compatible stream).
        let estimated_context_tokens =
            crate::model::context::estimate_context_tokens(&history, &tool_specs);
        match emit(
            &events,
            &cancellation,
            RunEvent::StepStarted {
                step,
                estimated_context_tokens,
            },
        )
        .await
        {
            Ok(()) => {}
            Err(Error::Cancelled) => {
                return commit_terminal_history(core, history, TerminalKind::Cancelled, &events)
                    .await;
            }
            Err(error) => return Err(error),
        }

        let mut control = RunControl {
            hooks,
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
                return commit_terminal(
                    core,
                    history,
                    error.capture,
                    TerminalKind::Cancelled,
                    &events,
                )
                .await;
            }
            Err(error) => {
                return commit_terminal(
                    core,
                    history,
                    error.capture,
                    TerminalKind::Failed(Error::from(error.error)),
                    &events,
                )
                .await;
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

        let mut tool_turn = StagedToolTurn::model_requested(tool_calls);
        let model_tool_result =
            run_staged_tool_turn(&core, &runtime, &mut tool_turn, &mut history, &mut control).await;
        history =
            match resolve_tool_turn_result(Arc::clone(&core), history, model_tool_result, &events)
                .await
            {
                Ok(history) => history,
                Err(terminal) => return *terminal,
            };
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

async fn run_staged_tool_turn(
    core: &Arc<SessionCore>,
    runtime: &Rho,
    tool_turn: &mut StagedToolTurn,
    history: &mut Vec<Message>,
    control: &mut RunControl<'_>,
) -> Result<ToolTurnStatus, Error> {
    let status = execute_staged_tool_turn(core, runtime, tool_turn, history, control).await?;
    if status.is_cancelled() {
        return Ok(status);
    }
    match apply_staged_steering(
        control.steering,
        history,
        control.events,
        control.cancellation,
    )
    .await
    {
        Ok(()) => Ok(ToolTurnStatus::Completed),
        Err(Error::Cancelled) => Ok(ToolTurnStatus::Cancelled),
        Err(error) => Err(error),
    }
}

/// Route a staged tool-turn result through the cooperative terminal commit policy.
///
/// `Ok(history)` means the turn completed and the loop should continue with that
/// candidate history. Any `Err` is the terminal result for `execute_turn_loop`.
async fn resolve_tool_turn_result(
    core: Arc<SessionCore>,
    history: Vec<Message>,
    result: Result<ToolTurnStatus, Error>,
    events: &mpsc::Sender<RunEvent>,
) -> Result<Vec<Message>, Box<Result<RunOutcome, Error>>> {
    match result {
        Ok(status) if status.is_cancelled() => Err(Box::new(
            commit_terminal_history(core, history, TerminalKind::Cancelled, events).await,
        )),
        Ok(_) => Ok(history),
        Err(Error::Cancelled) => Err(Box::new(
            commit_terminal_history(core, history, TerminalKind::Cancelled, events).await,
        )),
        // Event-consumer interrupts leave candidate history uninstalled.
        Err(error @ Error::Interrupted { .. }) => Err(Box::new(Err(error))),
        Err(error) => Err(Box::new(
            commit_terminal(
                core,
                history,
                StreamCapture::default(),
                TerminalKind::Failed(error),
                events,
            )
            .await,
        )),
    }
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
    let outcome = core
        .commit_compaction(previous, replacement.clone(), usage)?
        .with_committed_snapshot(core.persistence_snapshot());
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

impl RequestFailure {
    fn boxed(error: ProviderError, capture: StreamCapture) -> Box<Self> {
        Box::new(Self { error, capture })
    }
}

struct RunControl<'a> {
    hooks: &'a RunHooks,
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
) -> Result<(ModelResponse, StreamCapture), Box<RequestFailure>> {
    let mut next_attempt_index = 1;
    let mut provider_turn_attempts = 0;
    let mut invalid_responses = 0;
    let mut failed_requests = 0;
    loop {
        provider_turn_attempts += 1;
        let result = provider_turn(
            scope.runtime,
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
                let outcome = if response.protocol_issue().is_none() {
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
                let retry_after = failure.error.retry_after();
                let _ = emit(
                    control.events,
                    control.cancellation,
                    RunEvent::ProviderStreamReset {
                        reason: crate::ProviderStreamResetReason::RetryableFailure {
                            kind: failure.error.kind(),
                            retry_after: retry_after.filter(|delay| !delay.is_zero()),
                        },
                        detail,
                    },
                )
                .await;
                let exponential =
                    RETRYABLE_REQUEST_BASE_DELAY * 2u32.pow(failed_requests as u32 - 1);
                // Honor a provider wait when present. Cap so a multi-hour
                // Retry-After cannot stall the interactive session; the final
                // error still carries the full provider hint.
                let delay = match retry_after {
                    Some(wait) if !wait.is_zero() => {
                        wait.min(MAX_HONORED_RETRY_AFTER).max(exponential)
                    }
                    _ => exponential,
                };
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = control.cancellation.cancelled() => {
                        // ProviderStreamReset already abandoned this attempt.
                        // Do not commit its discarded partials as AbortedAssistant.
                        failure.capture = StreamCapture::default();
                        return Err(failure);
                    }
                }
                continue;
            }
        };
        let Some(issue) = response.protocol_issue() else {
            return Ok((response, capture));
        };
        invalid_responses += 1;
        if invalid_responses >= INVALID_RESPONSE_ATTEMPTS
            || provider_turn_attempts >= PROVIDER_TURN_ATTEMPTS
        {
            // Invalid attempts are discarded; do not install stream fragments
            // into session history on the terminal failure path.
            return Err(RequestFailure::boxed(
                ProviderError::new(
                    ProviderErrorKind::InvalidResponse,
                    issue,
                    Retryability::Permanent,
                ),
                StreamCapture::default(),
            ));
        }
        let detail = format!(
            "retrying malformed provider response after provider attempt {provider_turn_attempts} of {PROVIDER_TURN_ATTEMPTS}"
        );
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

async fn provider_turn(
    runtime: &Rho,
    history: &[Message],
    tools: &[crate::model::ToolSpec],
    accumulated_usage: &ModelUsage,
    reasoning_level: crate::ReasoningLevel,
    prompt_cache_key: Option<&str>,
    control: &mut RunControl<'_>,
) -> Result<(ModelResponse, StreamCapture), Box<RequestFailure>> {
    let provider = runtime.provider.as_ref();
    let (provider_events, mut receiver) =
        provider_event_channel(NonZeroUsize::new(PROVIDER_EVENT_CAPACITY).unwrap());
    let request = ModelRequest {
        messages: history,
        tools,
        cancellation: control.cancellation.clone(),
        reasoning_level,
        prompt_cache_key,
    };
    let request_options = runtime
        .service_tier
        .map(|tier| ModelRequestOptions::default().with_service_tier(tier))
        .unwrap_or_default();
    let cancellation_mode = provider.cancellation_mode();
    let identity = provider.identity();
    let profile = ModelCallProfile {
        provider: identity.provider.clone(),
        model: identity.model.clone(),
        reasoning: reasoning_level,
        service_tier: request_options.service_tier(),
    };
    let mut future =
        provider.send_turn_stream_with_options(request, request_options, provider_events);
    let mut capture = StreamCapture::default();
    let mut timer = ModelCallTimer::start(Instant::now());
    let mut stream_open = true;
    let mut commands_open = true;
    let result = loop {
        tokio::select! {
            result = &mut future => break (result, Instant::now()),
            event = receiver.recv_timed_stream_event(), if stream_open => {
                match event {
                    Some(event) => {
                        if let Err(error) = handle_timed_provider_stream_event(
                            event,
                            &mut timer,
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
                            return Err(RequestFailure::boxed(error, capture));
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
                return Err(RequestFailure::boxed(
                    ProviderError::interrupted("provider request cancelled"),
                    capture,
                ));
            }
        }
    };
    let (result, completed_at) = result;
    while let Some(event) = receiver.try_recv_timed_stream_event() {
        if let Err(error) = handle_timed_provider_stream_event(
            event,
            &mut timer,
            &identity,
            accumulated_usage,
            &mut capture,
            control.events,
            control.cancellation,
        )
        .await
        {
            if control.cancellation.is_cancelled() {
                drain_cancelled_provider_events(&mut receiver, &identity, &mut capture);
            }
            return Err(RequestFailure::boxed(error, capture));
        }
    }
    match result {
        Ok(response) => {
            let metrics = timer.finish(completed_at, capture.usage().output_tokens);
            if let Err(error) = emit(
                control.events,
                control.cancellation,
                RunEvent::ModelCallCompleted { profile, metrics },
            )
            .await
            {
                return Err(RequestFailure::boxed(
                    ProviderError::interrupted(error.to_string()),
                    capture,
                ));
            }
            Ok((response, capture))
        }
        Err(error) => Err(RequestFailure::boxed(error, capture)),
    }
}

async fn handle_timed_provider_stream_event(
    (event, observed_at): (crate::provider::ProviderStreamEvent, Option<Instant>),
    timer: &mut ModelCallTimer,
    identity: &crate::model::ModelIdentity,
    accumulated_usage: &ModelUsage,
    capture: &mut StreamCapture,
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
) -> Result<(), ProviderError> {
    match event {
        crate::provider::ProviderStreamEvent::Model(event) => {
            if let ModelEvent::GenerationOutputTokens(tokens) = event {
                timer.observe_generation_output_tokens(tokens);
                return Ok(());
            }
            timer.observe(&event, observed_at);
            handle_provider_event(
                event,
                identity,
                accumulated_usage,
                capture,
                events,
                cancellation,
            )
            .await
        }
        crate::provider::ProviderStreamEvent::Request(event) => {
            timer.discard_attempt_output(observed_at);
            handle_provider_request_event(event, capture, events, cancellation).await
        }
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
    emit(events, cancellation, RunEvent::ProviderRequestRetry)
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
    let Some(run_event) = capture_provider_event(event, identity, accumulated_usage, capture)
    else {
        return Ok(());
    };
    emit(events, cancellation, run_event)
        .await
        .map_err(|error| ProviderError::interrupted(error.to_string()))
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

#[cfg(test)]
#[path = "orchestration_tests.rs"]
mod tests;
