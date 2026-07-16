use std::{collections::BTreeMap, num::NonZeroUsize, sync::Arc};

use tokio::sync::mpsc;

use crate::{
    client::Rho,
    event::{RunOutcome, StopReason},
    model::{
        AbortedAssistant, AssistantMessage, ContentBlock, Message, ModelEvent, ModelRequest,
        ModelResponse, ModelUsage, PartialToolCall, ProviderContextBlock,
    },
    provider::{provider_event_channel, ModelProvider},
    run::RunCommand,
    session::{HistoryMetrics, SessionCore, SessionState, UserInput},
    steering::SteeringQueue,
    CancellationToken, Error, ProviderError, ProviderErrorKind, Retryability, RunEvent, RunId,
};

const PROVIDER_EVENT_CAPACITY: usize = 16;
const TOOL_PROGRESS_CAPACITY: usize = 16;
const INVALID_RESPONSE_ATTEMPTS: usize = 2;

mod tool_turn;

use tool_turn::{execute_tool, StagedToolTurn};

#[derive(Default)]
struct StreamCapture {
    text: String,
    reasoning: String,
    reasoning_summary: String,
    provider_context: Vec<ProviderContextBlock>,
    partial_tool_calls: BTreeMap<usize, PartialToolCall>,
    usage: ModelUsage,
}

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
        RunEvent::Started { run_id, revision },
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
        match maybe_compact(
            &core,
            &runtime,
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
        let (response, capture) = match request_valid_response(
            runtime.provider.as_ref(),
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
        accumulated_usage = accumulated_usage.saturating_add(&capture.usage);

        let ModelResponse::Assistant(content) = response;
        let tool_calls = content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall(call) => Some(call.clone()),
                ContentBlock::Text(_) | ContentBlock::Image(_) => None,
            })
            .collect::<Vec<_>>();
        let assistant = AssistantMessage {
            content,
            provenance: Some(runtime.provider.identity()),
            reasoning_summary: (!capture.reasoning_summary.is_empty())
                .then_some(capture.reasoning_summary),
            provider_context: capture.provider_context,
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
    runtime: &Rho,
    tool_specs: &[crate::model::ToolSpec],
    history: &mut Vec<Message>,
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<(), Error> {
    let Some(policy) = &runtime.compaction_policy else {
        return Ok(());
    };
    let context_tokens = crate::model::context::estimate_context_tokens(history, tool_specs);
    if !policy.should_compact(history.len(), context_tokens) {
        return Ok(());
    }
    let compactor = runtime
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
    let request = crate::CompactionRequest::new(history.clone(), cancellation.clone());
    let output = tokio::select! {
        result = compactor.compact(request) => result?,
        () = cancellation.cancelled() => return Err(Error::Cancelled),
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

async fn request_valid_response(
    provider: &dyn ModelProvider,
    history: &[Message],
    tools: &[crate::model::ToolSpec],
    accumulated_usage: &ModelUsage,
    reasoning_level: crate::ReasoningLevel,
    prompt_cache_key: Option<&str>,
    control: &mut RunControl<'_>,
) -> Result<(ModelResponse, StreamCapture), RequestFailure> {
    for attempt in 1..=INVALID_RESPONSE_ATTEMPTS {
        let (response, capture) = provider_turn(
            provider,
            history,
            tools,
            accumulated_usage,
            reasoning_level,
            prompt_cache_key,
            control,
        )
        .await?;
        if valid_response(&response) {
            return Ok((response, capture));
        }
        if attempt < INVALID_RESPONSE_ATTEMPTS {
            let _ = emit(
                control.events,
                control.cancellation,
                RunEvent::ProviderActivity {
                    kind: crate::PROVIDER_ACTIVITY_INVALID_RESPONSE_RETRY.into(),
                    detail: format!("retrying malformed provider response after attempt {attempt}"),
                },
            )
            .await;
        } else {
            return Err(RequestFailure {
                error: ProviderError::new(
                    ProviderErrorKind::InvalidResponse,
                    "provider returned an empty assistant response",
                    Retryability::Permanent,
                ),
                capture,
            });
        }
    }
    unreachable!("invalid response attempts is nonzero")
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
    let mut future = provider.send_turn_stream(request, provider_events);
    let mut capture = StreamCapture::default();
    let mut events_open = true;
    let mut commands_open = true;
    let result = loop {
        tokio::select! {
            result = &mut future => break result,
            event = receiver.recv(), if events_open => {
                match event {
                    Some(event) => {
                        if let Err(error) = handle_provider_event(
                            event,
                            provider.identity(),
                            accumulated_usage,
                            &mut capture,
                            control.events,
                            control.cancellation,
                        ).await {
                            return Err(RequestFailure { error, capture });
                        }
                    }
                    None => events_open = false,
                }
            }
            command = control.commands.recv(), if commands_open => {
                match command {
                    Some(command) => accept_non_tool_command(command, control.steering),
                    None => commands_open = false,
                }
            }
            () = control.cancellation.cancelled() => {
                return Err(RequestFailure {
                    error: ProviderError::interrupted("provider request cancelled"),
                    capture,
                });
            }
        }
    };
    while let Some(event) = receiver.try_recv() {
        if let Err(error) = handle_provider_event(
            event,
            provider.identity(),
            accumulated_usage,
            &mut capture,
            control.events,
            control.cancellation,
        )
        .await
        {
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

async fn handle_provider_event(
    event: ModelEvent,
    identity: crate::model::ModelIdentity,
    accumulated_usage: &ModelUsage,
    capture: &mut StreamCapture,
    events: &mpsc::Sender<RunEvent>,
    cancellation: &CancellationToken,
) -> Result<(), ProviderError> {
    let run_event = match event {
        ModelEvent::OutputDelta(text) => {
            capture.text.push_str(&text);
            RunEvent::AssistantTextDelta { text }
        }
        ModelEvent::ReasoningDelta(text) => {
            capture.reasoning.push_str(&text);
            RunEvent::ReasoningDelta { text }
        }
        ModelEvent::ReasoningSummaryDelta(text) => {
            capture.reasoning_summary.push_str(&text);
            RunEvent::ReasoningSummaryDelta { text }
        }
        ModelEvent::WebSearch(detail) => RunEvent::ProviderActivity {
            kind: crate::PROVIDER_ACTIVITY_WEB_SEARCH.into(),
            detail,
        },
        ModelEvent::ToolCallDelta {
            index,
            id,
            name,
            arguments,
        } => {
            let partial =
                capture
                    .partial_tool_calls
                    .entry(index)
                    .or_insert_with(|| PartialToolCall {
                        id: None,
                        name: None,
                        arguments: String::new(),
                    });
            if id.is_some() {
                partial.id.clone_from(&id);
            }
            if name.is_some() {
                partial.name.clone_from(&name);
            }
            partial.arguments.push_str(&arguments);
            RunEvent::ToolCallUpdated {
                index,
                id,
                name,
                arguments_delta: arguments,
            }
        }
        ModelEvent::ProviderContext {
            kind,
            position,
            data,
        } => {
            capture.provider_context.push(ProviderContextBlock {
                identity,
                kind: kind.clone(),
                position,
                data,
            });
            RunEvent::ProviderContextUpdated { kind }
        }
        ModelEvent::Usage(usage) => {
            // Providers may emit partial usage across multiple stream events
            // (for example Anthropic input/cache at message_start and later
            // output deltas). Merge within the turn instead of overwriting.
            capture.usage = capture.usage.saturating_add(&usage);
            RunEvent::UsageUpdated {
                usage: accumulated_usage.saturating_add(&capture.usage),
            }
        }
    };
    emit(events, cancellation, run_event)
        .await
        .map_err(|error| ProviderError::interrupted(error.to_string()))
}

async fn commit_cancellation(
    core: Arc<SessionCore>,
    mut history: Vec<Message>,
    capture: StreamCapture,
    events: &mpsc::Sender<RunEvent>,
) -> Result<RunOutcome, Error> {
    if !capture.text.is_empty()
        || !capture.reasoning_summary.is_empty()
        || !capture.provider_context.is_empty()
        || !capture.partial_tool_calls.is_empty()
        || capture.usage != ModelUsage::default()
    {
        let content = if capture.text.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock::Text(capture.text)]
        };
        history.push(Message::AbortedAssistant(Box::new(AbortedAssistant {
            content,
            reasoning: String::new(),
            provenance: None,
            reasoning_summary: (!capture.reasoning_summary.is_empty())
                .then_some(capture.reasoning_summary),
            provider_context: capture.provider_context,
            tool_calls: capture.partial_tool_calls.into_values().collect(),
            usage: capture.usage,
        })));
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
                detail: detail.to_owned(),
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
