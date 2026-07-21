use std::{future::Future, num::NonZeroUsize, pin::Pin, sync::Arc, task::Poll, time::Instant};

use tokio::sync::mpsc;

use crate::{
    host_input::HostInputEnvelope,
    model::ToolCall,
    orchestration::Rho,
    session::SessionCore,
    tool::{
        tool_progress_channel, Tool, ToolContext, ToolError, ToolInvocation, ToolInvocationSource,
        ToolPreparationContext,
    },
    CancellationToken, ToolCallId,
};

use super::{BatchCall, CallState};

type PreparationFuture<'tool, 'batch> =
    Pin<Box<dyn Future<Output = BatchCall<'tool>> + Send + 'batch>>;

struct PreparationScope<'a> {
    core: &'a Arc<SessionCore>,
    runtime: &'a Rho,
    cancellation: &'a CancellationToken,
    limit: NonZeroUsize,
}

pub(super) async fn prepare_batch<'a>(
    core: &Arc<SessionCore>,
    runtime: &Rho,
    tools: &'a [Option<Arc<dyn Tool>>],
    calls: Vec<(ToolCall, ToolCallId, ToolInvocationSource)>,
    cancellation: &CancellationToken,
    limit: NonZeroUsize,
) -> (Vec<BatchCall<'a>>, bool) {
    let interrupted = calls
        .iter()
        .map(|(call, id, _)| (call.clone(), id.clone()))
        .collect::<Vec<_>>();
    let scope = PreparationScope {
        core,
        runtime,
        cancellation,
        limit,
    };
    let mut preparations = calls
        .into_iter()
        .enumerate()
        .map(|(index, (call, id, source))| {
            Box::pin(prepare_call(
                &scope,
                tools[index].as_ref(),
                call,
                id,
                source,
            )) as PreparationFuture<'a, '_>
        })
        .map(Some)
        .collect::<Vec<_>>();
    let mut prepared = (0..preparations.len()).map(|_| None).collect::<Vec<_>>();

    let cancellation_wait = cancellation.cancelled();
    tokio::pin!(cancellation_wait);
    let cancelled = std::future::poll_fn(|cx| {
        for (index, future_slot) in preparations.iter_mut().enumerate() {
            let Some(future) = future_slot.as_mut() else {
                continue;
            };
            if let Poll::Ready(entry) = future.as_mut().poll(cx) {
                prepared[index] = Some(entry);
                *future_slot = None;
            }
        }
        if preparations.iter().all(Option::is_none) {
            return Poll::Ready(false);
        }
        if cancellation_wait.as_mut().poll(cx).is_ready() {
            return Poll::Ready(true);
        }
        Poll::Pending
    })
    .await;

    if cancelled {
        return (
            interrupted
                .into_iter()
                .map(|(call, id)| interrupted_entry(call, id))
                .collect(),
            true,
        );
    }
    (
        prepared
            .into_iter()
            .map(|entry| entry.expect("completed preparation has an entry"))
            .collect(),
        false,
    )
}

async fn prepare_call<'a>(
    scope: &PreparationScope<'_>,
    tool: Option<&'a Arc<dyn Tool>>,
    call: ToolCall,
    id: ToolCallId,
    source: ToolInvocationSource,
) -> BatchCall<'a> {
    let Some(tool) = tool else {
        return BatchCall {
            call,
            id,
            state: CallState::Unavailable,
            progress: None,
            host_input: None,
            dependencies: Vec::new(),
            queued_at: Instant::now(),
            execution_started: None,
            result: None,
        };
    };
    let (context, progress, host_input) = execution_context(
        scope.core,
        scope.runtime,
        &id,
        scope.cancellation,
        scope.limit,
    );
    let invocation = match source {
        ToolInvocationSource::Model => ToolInvocation::new(id.clone(), call.arguments.clone()),
        ToolInvocationSource::Host => ToolInvocation::from_host(id.clone(), call.arguments.clone()),
    };
    let state = match tool
        .prepare(
            invocation,
            ToolPreparationContext::new(
                scope.runtime.workspace.clone(),
                scope.cancellation.clone(),
            ),
        )
        .await
    {
        Ok(invocation) => CallState::Prepared {
            invocation: Some(invocation),
            context,
        },
        Err(error) => CallState::PreparationFailed(error),
    };
    BatchCall {
        call,
        id,
        state,
        progress: Some(progress),
        host_input: Some(host_input),
        dependencies: Vec::new(),
        queued_at: Instant::now(),
        execution_started: None,
        result: None,
    }
}

fn interrupted_entry<'a>(call: ToolCall, id: ToolCallId) -> BatchCall<'a> {
    BatchCall {
        call,
        id,
        state: CallState::PreparationFailed(ToolError::cancelled()),
        progress: None,
        host_input: None,
        dependencies: Vec::new(),
        queued_at: Instant::now(),
        execution_started: None,
        result: None,
    }
}

fn execution_context(
    core: &Arc<SessionCore>,
    runtime: &Rho,
    call_id: &ToolCallId,
    cancellation: &CancellationToken,
    limit: NonZeroUsize,
) -> (
    ToolContext,
    crate::tool::ToolProgressReceiver,
    mpsc::Receiver<HostInputEnvelope>,
) {
    let (progress, progress_receiver) = tool_progress_channel(limit);
    let (host_input, host_input_receiver) =
        crate::host_input::channel(limit.get(), cancellation.clone());
    let context = ToolContext::with_security(
        runtime.workspace.clone(),
        Arc::clone(&runtime.workspace_policy),
        Arc::clone(&runtime.approval_handler),
        core.approvals(),
        Arc::clone(&runtime.approval_audit),
        cancellation.clone(),
        progress,
    )
    .with_call_id(call_id.clone())
    .with_host_input(host_input);
    (context, progress_receiver, host_input_receiver)
}
