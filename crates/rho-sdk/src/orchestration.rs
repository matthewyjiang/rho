use std::{num::NonZeroUsize, sync::Arc, time::Instant};

use tokio::sync::mpsc;

use crate::{
    client::Rho,
    event::{RunOutcome, StopReason},
    model::{
        AssistantMessage, ContentBlock, Message, ModelEvent, ModelRequest, ModelResponse,
        ModelUsage,
    },
    provider::{
        provider_event_channel, provider_steering_channel, ModelRequestOptions,
        ProviderCancellationMode,
    },
    run::RunCommand,
    session::{HistoryMetrics, RunStart, SessionCore, SessionState},
    steering::SteeringQueue,
    CancellationToken, Error, ModelCallProfile, ProviderError, RunEvent, RunId,
};

const PROVIDER_EVENT_CAPACITY: usize = 16;
pub(super) const INVALID_RESPONSE_ATTEMPTS: usize = 2;
/// Maximum logical provider requests for one model turn, including malformed
/// responses and retryable failures.
pub(super) const PROVIDER_TURN_ATTEMPTS: usize = 4;
/// Backoff before the first retryable-failure retry; doubles per retry.
pub(super) const RETRYABLE_REQUEST_BASE_DELAY: std::time::Duration =
    std::time::Duration::from_secs(1);
/// Upper bound when honoring provider `Retry-After` during auto-retry.
///
/// Keeps interactive turns from blocking on multi-hour quota resets. The full
/// provider wait still appears on the reset event and final error.
pub(super) const MAX_HONORED_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

mod async_jobs;
mod model_call_timer;
mod provider_cancellation;
mod provider_request;
mod run_hooks;
mod steering_control;
mod stream_capture;
mod terminal;
mod tool_batch;
mod tool_turn;

use async_jobs::{
    await_all_jobs, await_first_job, forward_job_notice, harvest_ready_jobs, split_tool_calls,
    AsyncJobSet, AwaitJobs,
};
use model_call_timer::ModelCallTimer;
use provider_cancellation::{
    drain_cancelled_provider_events, drain_cooperative_provider_on_cancellation,
};
use provider_request::{request_valid_response, ProviderRequestScope, RequestFailure};
use run_hooks::RunHooks;
pub(in crate::orchestration) use steering_control::{
    accept_command as accept_non_tool_command, apply_staged as apply_staged_steering,
    drain_commands, handle_outcome as handle_steering_outcome,
};
use stream_capture::{capture_provider_event, StreamCapture};
use terminal::{commit_terminal, commit_terminal_history, send_terminal, TerminalKind};
use tool_turn::{
    final_assistant_content, resolve_tool_turn_result, run_staged_tool_turn, StagedToolTurn,
};

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
    let mut async_jobs = AsyncJobSet::new(runtime.max_parallel_tools);
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
            async_jobs: &mut async_jobs,
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
        {
            let mut control = RunControl {
                hooks,
                cancellation: &cancellation,
                events: &events,
                commands: &mut commands,
                steering: &mut steering,
                async_jobs: &mut async_jobs,
            };
            if let Err(error) = harvest_ready_jobs(&mut control).await {
                return terminate_run(
                    core,
                    history,
                    &mut async_jobs,
                    hooks,
                    &events,
                    &cancellation,
                    error,
                )
                .await;
            }
        }
        async_jobs.drain_finished(&mut history);
        let request_scope = ProviderRequestScope {
            runtime: &runtime,
            session_id: core.id(),
            run_id: &run_id,
            step_index: step,
        };
        if !async_jobs.has_pending() {
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
                Err(error) => {
                    return terminate_run(
                        core,
                        history,
                        &mut async_jobs,
                        hooks,
                        &events,
                        &cancellation,
                        error,
                    )
                    .await;
                }
            }
        } else if runtime.compaction_policy.is_some() {
            tracing::warn!(
                pending = async_jobs.pending_count(),
                "skipping compaction while async tool jobs are pending"
            );
        }
        drain_commands(&mut commands, &mut steering);
        // Delivered steers stay staged so a Reuse continuation still matches the
        // suffix the server already prepended. Undelivered steers accepted
        // between steps (including during compact) are applied before the next
        // request so default providers do not spend a turn just to release them.
        if !steering.has_delivered() {
            match apply_staged_steering(&mut steering, &mut history, &events, &cancellation).await {
                Ok(()) => {}
                Err(error) => {
                    return terminate_run(
                        core,
                        history,
                        &mut async_jobs,
                        hooks,
                        &events,
                        &cancellation,
                        error,
                    )
                    .await;
                }
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
            Err(error) => {
                return terminate_run(
                    core,
                    history,
                    &mut async_jobs,
                    hooks,
                    &events,
                    &cancellation,
                    error,
                )
                .await;
            }
        }

        let mut control = RunControl {
            hooks,
            cancellation: &cancellation,
            events: &events,
            commands: &mut commands,
            steering: &mut steering,
            async_jobs: &mut async_jobs,
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
                control
                    .async_jobs
                    .interrupt(&mut history, hooks, &events)
                    .await;
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
                control
                    .async_jobs
                    .interrupt(&mut history, hooks, &events)
                    .await;
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
        let async_ids = capture.async_call_ids();
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
        let (async_calls, sync_calls) = split_tool_calls(tool_calls, &async_ids, &runtime.tools);
        let spawned_async = !async_calls.is_empty();
        core.publish_in_flight_history(&history);
        if let Err(error) = control
            .async_jobs
            .spawn(
                async_calls,
                &core,
                &runtime,
                control.hooks,
                control.events,
                control.cancellation,
            )
            .await
        {
            return terminate_run(
                core,
                history,
                control.async_jobs,
                hooks,
                &events,
                &cancellation,
                error,
            )
            .await;
        }

        // Detached jobs may keep using shared resources for their lifetime.
        // Wait before entering the synchronous scheduler, which may run an
        // exclusive plan and must not overlap that work.
        if !sync_calls.is_empty() {
            if control.async_jobs.has_pending() {
                if let Err(error) = await_all_jobs(&mut control).await {
                    return terminate_run(
                        core,
                        history,
                        control.async_jobs,
                        hooks,
                        &events,
                        &cancellation,
                        error,
                    )
                    .await;
                }
            }
            control.async_jobs.drain_finished(&mut history);
        }

        if !sync_calls.is_empty() || (!spawned_async && was_steered) {
            let mut tool_turn = StagedToolTurn::model_requested(sync_calls);
            let model_tool_result =
                run_staged_tool_turn(&core, &runtime, &mut tool_turn, &mut history, &mut control)
                    .await;
            let should_interrupt_jobs = match &model_tool_result {
                Ok(status) => status.is_cancelled(),
                Err(_) => true,
            };
            if should_interrupt_jobs {
                control
                    .async_jobs
                    .interrupt(&mut history, hooks, &events)
                    .await;
            }
            history = match resolve_tool_turn_result(
                Arc::clone(&core),
                history,
                model_tool_result,
                &events,
            )
            .await
            {
                Ok(history) => history,
                Err(terminal) => return *terminal,
            };
            continue;
        }
        if spawned_async {
            if let Err(error) = apply_staged_steering(
                control.steering,
                &mut history,
                control.events,
                control.cancellation,
            )
            .await
            {
                return terminate_run(
                    core,
                    history,
                    control.async_jobs,
                    hooks,
                    &events,
                    &cancellation,
                    error,
                )
                .await;
            }
            continue;
        }

        if let Err(error) = harvest_ready_jobs(&mut control).await {
            return terminate_run(
                core,
                history,
                control.async_jobs,
                hooks,
                &events,
                &cancellation,
                error,
            )
            .await;
        }
        if control.async_jobs.drain_finished(&mut history) > 0 {
            continue;
        }
        if control.async_jobs.has_pending() {
            match await_first_job(&mut control).await {
                Ok(AwaitJobs::Continue) => {
                    if let Err(error) = harvest_ready_jobs(&mut control).await {
                        return terminate_run(
                            core,
                            history,
                            control.async_jobs,
                            hooks,
                            &events,
                            &cancellation,
                            error,
                        )
                        .await;
                    }
                    control.async_jobs.drain_finished(&mut history);
                    if let Err(error) = apply_staged_steering(
                        control.steering,
                        &mut history,
                        control.events,
                        control.cancellation,
                    )
                    .await
                    {
                        return terminate_run(
                            core,
                            history,
                            control.async_jobs,
                            hooks,
                            &events,
                            &cancellation,
                            error,
                        )
                        .await;
                    }
                    continue;
                }
                Ok(AwaitJobs::Cancelled) => {
                    return terminate_run(
                        core,
                        history,
                        control.async_jobs,
                        hooks,
                        &events,
                        &cancellation,
                        Error::Cancelled,
                    )
                    .await;
                }
                Err(error) => {
                    return terminate_run(
                        core,
                        history,
                        control.async_jobs,
                        hooks,
                        &events,
                        &cancellation,
                        error,
                    )
                    .await;
                }
            }
        }

        let content = final_assistant_content(&history);
        let revision = core.commit(history)?;
        let outcome = RunOutcome::new(content, accumulated_usage, StopReason::EndTurn, revision);
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

    async_jobs.interrupt(&mut history, hooks, &events).await;
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

/// Interrupts pending async jobs, then commits or returns the terminal error.
///
/// Event-consumer interrupts stay uncommitted. Every terminal path that would
/// otherwise copy this match goes through here so interrupted jobs emit
/// `ToolFinished`.
async fn terminate_run(
    core: Arc<SessionCore>,
    mut history: Vec<Message>,
    async_jobs: &mut AsyncJobSet,
    hooks: &RunHooks,
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
    error: Error,
) -> Result<RunOutcome, Error> {
    async_jobs.interrupt(&mut history, hooks, events).await;
    match error {
        Error::Cancelled => {
            commit_terminal_history(core, history, TerminalKind::Cancelled, events).await
        }
        Error::Interrupted { .. } => Err(error),
        error => {
            commit_terminal(
                core,
                history,
                StreamCapture::default(),
                TerminalKind::Failed(error),
                events,
            )
            .await
        }
    }
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
        .with_trigger(crate::CompactionTrigger::Automatic)
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

pub(super) struct RunControl<'a> {
    hooks: &'a RunHooks,
    cancellation: &'a CancellationToken,
    events: &'a mpsc::Sender<RunEvent>,
    commands: &'a mut mpsc::Receiver<RunCommand>,
    steering: &'a mut SteeringQueue,
    async_jobs: &'a mut AsyncJobSet,
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
    let (offer_tx, steering_rx) = provider_steering_channel();
    let mut offer_tx = Some(offer_tx);
    let (outcomes_tx, mut outcomes_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut future =
        provider.send_turn_stream_steerable(request, request_options, provider_events, steering_rx);
    control.steering.offer_into(&mut offer_tx, &outcomes_tx);
    let mut capture = StreamCapture::default();
    let mut timer = ModelCallTimer::start(Instant::now());
    let mut stream_open = true;
    let mut commands_open = true;
    let mut outcomes_open = true;
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
                            control.steering.reset_delivery();
                            return Err(RequestFailure::boxed(error, capture));
                        }
                    }
                    None => stream_open = false,
                }
            }
            command = control.commands.recv(), if commands_open => {
                match command {
                    Some(command) => {
                        accept_non_tool_command(command, control.steering);
                        control.steering.offer_into(&mut offer_tx, &outcomes_tx);
                    }
                    None => commands_open = false,
                }
            }
            outcome = outcomes_rx.recv(), if outcomes_open => {
                match outcome {
                    Some(outcome) => {
                        if let Err(error) = handle_steering_outcome(
                            outcome,
                            control.steering,
                            control.events,
                            control.cancellation,
                        ).await {
                            drop(future);
                            control.steering.reset_delivery();
                            return Err(RequestFailure::boxed(
                                ProviderError::interrupted(error.to_string()),
                                capture,
                            ));
                        }
                    }
                    None => outcomes_open = false,
                }
            }
            notice = control.async_jobs.poll_event() => {
                if let Err(error) = forward_job_notice(
                    notice,
                    control.async_jobs,
                    control.hooks,
                    control.events,
                    control.cancellation,
                ).await {
                    drop(future);
                    control.steering.reset_delivery();
                    return Err(RequestFailure::boxed(
                        ProviderError::interrupted(error.to_string()),
                        capture,
                    ));
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
                control.steering.reset_delivery();
                return Err(RequestFailure::boxed(
                    ProviderError::interrupted("provider request cancelled"),
                    capture,
                ));
            }
        }
    };
    let (result, completed_at) = result;
    drop(offer_tx);
    drop(outcomes_tx);
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
            control.steering.reset_delivery();
            return Err(RequestFailure::boxed(error, capture));
        }
    }
    while let Ok(outcome) = outcomes_rx.try_recv() {
        if let Err(error) = handle_steering_outcome(
            outcome,
            control.steering,
            control.events,
            control.cancellation,
        )
        .await
        {
            control.steering.reset_delivery();
            return Err(RequestFailure::boxed(
                ProviderError::interrupted(error.to_string()),
                capture,
            ));
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
                control.steering.reset_delivery();
                return Err(RequestFailure::boxed(
                    ProviderError::interrupted(error.to_string()),
                    capture,
                ));
            }
            Ok((response, capture))
        }
        Err(error) => {
            control.steering.reset_delivery();
            Err(RequestFailure::boxed(error, capture))
        }
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

pub(super) async fn emit(
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
#[path = "orchestration_async_tests.rs"]
mod async_tests;
#[cfg(test)]
#[path = "orchestration_tests.rs"]
mod tests;
