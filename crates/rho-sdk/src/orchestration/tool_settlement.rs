//! Terminal tool settlement shared by synchronous and detached schedulers.

use crate::{
    event::{ToolCompletion, ToolFailure},
    model::{ToolCall, ToolResult},
    tool::{ToolError, ToolErrorKind, ToolOutput},
};

pub(super) const INTERRUPTED_TOOL_RESULT_CONTENT: &str = "tool call interrupted before completion";

pub(super) fn interrupted_result(call: &ToolCall) -> ToolResult {
    ToolResult {
        id: call.id.clone(),
        ok: false,
        content: INTERRUPTED_TOOL_RESULT_CONTENT.into(),
    }
}

/// Terminal settlement preserves completed output, but cancellation means interrupted.
pub(super) fn settled(
    call: &ToolCall,
    result: Result<ToolOutput, ToolError>,
) -> Option<(ToolResult, ToolCompletion)> {
    if matches!(&result, Err(error) if error.kind() == ToolErrorKind::Cancelled) {
        return None;
    }
    let completion = ToolCompletion::from_result(result);
    Some((completion.model_result(&call.name, &call.id), completion))
}

pub(super) fn interrupted(call: &ToolCall) -> (ToolResult, ToolCompletion) {
    let result = interrupted_result(call);
    let completion = ToolCompletion::Failure(ToolFailure::new(
        ToolErrorKind::Cancelled,
        result.content.clone(),
    ));
    (result, completion)
}
