use std::{collections::BTreeMap, num::NonZeroUsize, sync::Arc};

use tokio::sync::mpsc;

use crate::{
    client::Rho,
    event::{RunOutcome, StopReason, ToolCompletion, ToolFailure},
    model::{
        AbortedAssistant, AssistantMessage, ContentBlock, Message, ModelEvent, ModelRequest,
        ModelResponse, ModelUsage, PartialToolCall, ProviderContextBlock, ToolCall, ToolResult,
    },
    provider::{provider_event_channel, ModelProvider},
    session::{SessionCore, SessionState, UserInput},
    tool::{tool_progress_channel, ToolContext, ToolInvocation},
    CancellationToken, Error, ProviderError, ProviderErrorKind, Retryability, RunEvent, RunId,
    ToolCallId,
};

const PROVIDER_EVENT_CAPACITY: usize = 16;
const TOOL_PROGRESS_CAPACITY: usize = 16;
const INVALID_RESPONSE_ATTEMPTS: usize = 2;

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
) -> Result<RunOutcome, Error> {
    let (mut history, revision) = core.snapshot();
    history.push(Message::User(input.into_blocks()));
    emit(
        &events,
        &cancellation,
        RunEvent::Started { run_id, revision },
    )
    .await?;

    let mut accumulated_usage = ModelUsage::default();
    for step in 1..=runtime.max_steps.get() {
        match maybe_compact(&core, &runtime, &mut history, &cancellation, &events).await {
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
        emit(&events, &cancellation, RunEvent::StepStarted { step }).await?;

        let (response, capture) = match request_valid_response(
            runtime.provider.as_ref(),
            &history,
            &runtime.tools.specs(),
            &accumulated_usage,
            &cancellation,
            &events,
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
            content: content.clone(),
            provenance: Some(runtime.provider.identity()),
            reasoning_summary: (!capture.reasoning_summary.is_empty())
                .then_some(capture.reasoning_summary),
            provider_context: capture.provider_context,
        };
        history.push(Message::assistant(assistant));

        if tool_calls.is_empty() {
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

        for call in tool_calls {
            emit(
                &events,
                &cancellation,
                RunEvent::ToolProposed { call: call.clone() },
            )
            .await?;
            let result = match execute_tool(&runtime, &call, &cancellation, &events).await {
                Ok(result) => result,
                Err(Error::Cancelled) => {
                    return commit_cancelled_history(core, history, &events).await;
                }
                Err(error) => {
                    core.set_state(SessionState::Failed);
                    emit_failure(&events, &error).await;
                    return Err(error);
                }
            };
            history.push(Message::ToolResult(result));
        }
    }

    let error = Error::Provider(ProviderError::new(
        ProviderErrorKind::InvalidResponse,
        format!("provider exceeded {} model steps", runtime.max_steps),
        Retryability::Permanent,
    ));
    core.set_state(SessionState::Failed);
    emit_failure(&events, &error).await;
    Err(error)
}

async fn maybe_compact(
    core: &Arc<SessionCore>,
    runtime: &Rho,
    history: &mut Vec<Message>,
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<(), Error> {
    let Some(policy) = &runtime.compaction_policy else {
        return Ok(());
    };
    if !policy.should_compact(history.len()) {
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
    let request = crate::CompactionRequest::new(history.clone(), cancellation.clone());
    let output = tokio::select! {
        result = compactor.compact(request) => result?,
        () = cancellation.cancelled() => return Err(Error::Cancelled),
    };
    let replacement = output.into_messages();
    let outcome = core.commit_compaction(replacement.clone())?;
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

async fn request_valid_response(
    provider: &dyn ModelProvider,
    history: &[Message],
    tools: &[crate::model::ToolSpec],
    accumulated_usage: &ModelUsage,
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<(ModelResponse, StreamCapture), RequestFailure> {
    for attempt in 1..=INVALID_RESPONSE_ATTEMPTS {
        let (response, capture) = provider_turn(
            provider,
            history,
            tools,
            accumulated_usage,
            cancellation,
            events,
        )
        .await?;
        if valid_response(&response) {
            return Ok((response, capture));
        }
        if attempt < INVALID_RESPONSE_ATTEMPTS {
            let _ = emit(
                events,
                cancellation,
                RunEvent::ProviderActivity {
                    kind: "invalid_response_retry".into(),
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
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<(ModelResponse, StreamCapture), RequestFailure> {
    let (provider_events, mut receiver) =
        provider_event_channel(NonZeroUsize::new(PROVIDER_EVENT_CAPACITY).unwrap());
    let request = ModelRequest {
        messages: history,
        tools,
        cancellation: cancellation.clone(),
        prompt_cache_key: None,
    };
    let mut future = provider.send_turn_stream(request, provider_events);
    let mut capture = StreamCapture::default();
    let result = loop {
        tokio::select! {
            result = &mut future => break result,
            event = receiver.recv() => {
                if let Some(event) = event {
                    if let Err(error) = handle_provider_event(
                        event,
                        provider.identity(),
                        accumulated_usage,
                        &mut capture,
                        events,
                        cancellation,
                    ).await {
                        return Err(RequestFailure { error, capture });
                    }
                }
            }
            () = cancellation.cancelled() => {
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
            events,
            cancellation,
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
            kind: "web_search".into(),
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
            capture.usage = usage;
            RunEvent::UsageUpdated {
                usage: accumulated_usage.saturating_add(&capture.usage),
            }
        }
    };
    emit(events, cancellation, run_event)
        .await
        .map_err(|error| ProviderError::interrupted(error.to_string()))
}

async fn execute_tool(
    runtime: &Rho,
    call: &ToolCall,
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<ToolResult, Error> {
    let call_id = ToolCallId::from_string(call.id.clone()).map_err(|error| {
        Error::Provider(ProviderError::new(
            ProviderErrorKind::InvalidResponse,
            error.to_string(),
            Retryability::Permanent,
        ))
    })?;
    let Some(tool) = runtime.tools.get(&call.name) else {
        emit(
            events,
            cancellation,
            RunEvent::ToolFinished {
                call_id,
                result: ToolCompletion::Unavailable,
            },
        )
        .await?;
        return Ok(ToolResult {
            id: call.id.clone(),
            ok: false,
            content: format!("tool '{}' is unavailable", call.name),
        });
    };

    emit(
        events,
        cancellation,
        RunEvent::ToolStarted {
            call_id: call_id.clone(),
            name: call.name.clone(),
            metadata: crate::tool::ToolMetadata::default(),
        },
    )
    .await?;
    let (progress, mut progress_receiver) =
        tool_progress_channel(NonZeroUsize::new(TOOL_PROGRESS_CAPACITY).unwrap());
    let invocation = ToolInvocation::new(call_id.clone(), call.arguments.clone());
    let context = ToolContext::with_security(
        runtime.workspace.clone(),
        Arc::clone(&runtime.workspace_policy),
        Arc::clone(&runtime.approval_handler),
        cancellation.clone(),
        progress,
    );
    let mut future = tool.call(invocation, context);
    let result = loop {
        tokio::select! {
            result = &mut future => break result,
            progress = progress_receiver.recv() => {
                if let Some(progress) = progress {
                    emit(
                        events,
                        cancellation,
                        RunEvent::ToolUpdated {
                            call_id: call_id.clone(),
                            progress,
                        },
                    ).await?;
                }
            }
            () = cancellation.cancelled() => return Err(Error::Cancelled),
        }
    };
    while let Some(progress) = progress_receiver.try_recv() {
        emit(
            events,
            cancellation,
            RunEvent::ToolUpdated {
                call_id: call_id.clone(),
                progress,
            },
        )
        .await?;
    }

    match result {
        Ok(output) => {
            emit(
                events,
                cancellation,
                RunEvent::ToolFinished {
                    call_id,
                    result: ToolCompletion::Success(output.clone()),
                },
            )
            .await?;
            Ok(ToolResult {
                id: call.id.clone(),
                ok: true,
                content: output.content().to_owned(),
            })
        }
        Err(error) => {
            emit(
                events,
                cancellation,
                RunEvent::ToolFinished {
                    call_id,
                    result: ToolCompletion::Failure(ToolFailure::new(
                        error.kind(),
                        error.message().to_owned(),
                    )),
                },
            )
            .await?;
            Ok(ToolResult {
                id: call.id.clone(),
                ok: false,
                content: error.message().to_owned(),
            })
        }
    }
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
