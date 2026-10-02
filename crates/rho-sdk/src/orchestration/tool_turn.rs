use std::{collections::BTreeSet, sync::Arc};

use tokio::sync::mpsc;

use crate::{
    event::RunOutcome,
    model::{ContentBlock, Message, ToolCall},
    session::SessionCore,
    tool::{Tool, ToolInvocationSource},
    Error, RunEvent, ToolCallId,
};

use super::{
    apply_staged_steering, stream_capture::StreamCapture, terminal::commit_terminal,
    terminal::commit_terminal_history, terminal::TerminalKind, tool_batch,
    tool_settlement::interrupted_result, Rho, RunControl,
};

/// Pair calls already present in history when cancellation or failure prevents
/// them from entering the synchronous scheduler, which normally owns cleanup.
pub(super) fn interrupt_unstarted_calls(calls: Vec<StagedCall>, history: &mut Vec<Message>) {
    history.extend(
        calls
            .iter()
            .map(|entry| Message::ToolResult(interrupted_result(&entry.call))),
    );
}

pub(super) struct StagedCall {
    pub(super) call: ToolCall,
    pub(super) id: ToolCallId,
    pub(super) source: ToolInvocationSource,
    pub(super) tool: Option<Arc<dyn Tool>>,
}

pub(super) struct AsyncToolCall {
    pub(super) call: ToolCall,
    pub(super) id: ToolCallId,
    pub(super) tool: Arc<dyn Tool>,
}

impl StagedCall {
    pub(super) fn new(
        call: ToolCall,
        source: ToolInvocationSource,
        tool: Option<Arc<dyn Tool>>,
    ) -> Self {
        let id = ToolCallId::from_string(call.id.clone())
            .expect("validated provider tool call ID is nonempty");
        Self {
            call,
            id,
            source,
            tool,
        }
    }
}

/// Resolve against the advertisement snapshot, before choosing a scheduler.
pub(super) fn split_tool_calls(
    calls: Vec<ToolCall>,
    async_ids: &BTreeSet<String>,
    runtime: &Rho,
    advertised: &[crate::model::ToolSpec],
) -> (Vec<AsyncToolCall>, Vec<StagedCall>) {
    let mut async_calls = Vec::new();
    let mut sync_calls = Vec::new();
    for call in calls {
        let tool = advertised
            .iter()
            .any(|spec| spec.name == call.name)
            .then(|| runtime.tools.get(&call.name))
            .flatten();
        let staged = StagedCall::new(call, ToolInvocationSource::Model, tool);
        match staged {
            StagedCall {
                call,
                id,
                tool: Some(tool),
                ..
            } if async_ids.contains(&call.id)
                && tool.execution_mode() == crate::tool::ToolExecutionMode::Async =>
            {
                async_calls.push(AsyncToolCall { call, id, tool });
            }
            staged => sync_calls.push(staged),
        }
    }
    (async_calls, sync_calls)
}

pub(super) struct StagedToolTurn {
    calls: Vec<StagedCall>,
}

impl StagedToolTurn {
    pub(super) fn model_requested(calls: Vec<StagedCall>) -> Self {
        Self { calls }
    }

    pub(super) fn host_requested(call: ToolCall, runtime: &Rho) -> Self {
        let tool = runtime.tools.get(&call.name);
        Self {
            calls: vec![StagedCall::new(call, ToolInvocationSource::Host, tool)],
        }
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

pub(super) async fn run_staged_tool_turn(
    core: &Arc<SessionCore>,
    runtime: &Rho,
    tool_turn: &mut StagedToolTurn,
    history: &mut Vec<Message>,
    control: &mut RunControl<'_>,
) -> Result<ToolTurnStatus, Error> {
    let cancelled = tool_batch::execute(
        core,
        runtime,
        std::mem::take(&mut tool_turn.calls),
        history,
        control,
    )
    .await?;
    let status = if cancelled {
        ToolTurnStatus::Cancelled
    } else {
        ToolTurnStatus::Completed
    };
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
pub(super) async fn resolve_tool_turn_result(
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
pub(super) fn final_assistant_content(history: &[Message]) -> Vec<ContentBlock> {
    history
        .iter()
        .rev()
        .find_map(Message::completed_assistant_content)
        .map(<[ContentBlock]>::to_vec)
        .unwrap_or_default()
}
