use std::sync::Arc;

use crate::{
    model::{Message, ToolCall},
    session::SessionCore,
    tool::ToolInvocationSource,
    Error, ToolCallId,
};

use super::{tool_batch, Rho, RunControl};

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

pub(super) async fn execute_staged_tool_turn(
    core: &Arc<SessionCore>,
    runtime: &Rho,
    tool_turn: &mut StagedToolTurn,
    history: &mut Vec<Message>,
    control: &mut RunControl<'_>,
) -> Result<ToolTurnStatus, Error> {
    let cancelled =
        tool_batch::execute(core, runtime, tool_turn.take_calls(), history, control).await?;
    Ok(if cancelled {
        ToolTurnStatus::Cancelled
    } else {
        ToolTurnStatus::Completed
    })
}
