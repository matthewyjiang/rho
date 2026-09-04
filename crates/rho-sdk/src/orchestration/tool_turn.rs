use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    event::RunOutcome,
    model::{ContentBlock, Message, ToolCall},
    session::SessionCore,
    tool::ToolInvocationSource,
    Error, RunEvent, ToolCallId,
};

use super::{
    apply_staged_steering, stream_capture::StreamCapture, terminal::commit_terminal,
    terminal::commit_terminal_history, terminal::TerminalKind, tool_batch, Rho, RunControl,
};

#[cfg(test)]
pub(super) use tool_batch::INTERRUPTED_TOOL_RESULT_CONTENT;

pub(super) struct StagedToolTurn {
    calls: Vec<(ToolCall, ToolCallId, ToolInvocationSource)>,
}

impl StagedToolTurn {
    pub(super) fn model_requested(calls: Vec<ToolCall>) -> Self {
        Self::from_calls(calls, ToolInvocationSource::Model)
    }

    pub(super) fn host_requested(call: ToolCall) -> Self {
        Self::from_calls(vec![call], ToolInvocationSource::Host)
    }

    fn from_calls(calls: Vec<ToolCall>, source: ToolInvocationSource) -> Self {
        let calls = calls
            .into_iter()
            .map(|call| {
                let id = ToolCallId::from_string(call.id.clone())
                    .expect("validated provider tool call ID is nonempty");
                (call, id, source)
            })
            .collect();
        Self { calls }
    }

    fn take_calls(&mut self) -> Vec<(ToolCall, ToolCallId, ToolInvocationSource)> {
        std::mem::take(&mut self.calls)
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
    let cancelled =
        tool_batch::execute(core, runtime, tool_turn.take_calls(), history, control).await?;
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
