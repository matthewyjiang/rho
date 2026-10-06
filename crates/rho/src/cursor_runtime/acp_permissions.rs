//! Cursor-specific classification; generic ACP code selects the offered
//! allow_once/reject_once option and never persists an approval.

use agent_client_protocol::schema::v1::{RequestPermissionRequest, ToolKind};

use crate::{acp_runtime::permission::PermissionDecision, permission::PermissionMode};

use super::acp_config::{CursorCategory, CursorFence};

const CURSOR_WEB_SEARCH_ID_PREFIX: &str = "web_search_";

/// Fail closed outside Bypass. The ask-question permission fallback has one
/// allow_once per answer; rejecting selects its reject_once skip instead.
pub(crate) fn decide(
    fence: &CursorFence,
    mode: PermissionMode,
    request: &RequestPermissionRequest,
) -> PermissionDecision {
    match mode {
        PermissionMode::Plan
        | PermissionMode::Auto
        | PermissionMode::AllowEdits
        | PermissionMode::Supervised => return PermissionDecision::Reject,
        PermissionMode::Bypass => {}
    }
    if request
        .options
        .iter()
        .any(|option| option.option_id.0.as_ref() == "__ask_question_skip__")
    {
        return PermissionDecision::Reject;
    }
    // Cursor asks for web search with kind `search` (same as grep/glob) and a
    // synthetic `web_search_<queryId>` id (cursor-agent 2026.10.01,
    // `requestWebPermission`). It is network access, so gate it as Fetch.
    let web_search = request
        .tool_call
        .tool_call_id
        .0
        .starts_with(CURSOR_WEB_SEARCH_ID_PREFIX);
    let category = match request.tool_call.fields.kind {
        Some(ToolKind::Search) if web_search => CursorCategory::Fetch,
        Some(ToolKind::Execute) => CursorCategory::Shell,
        Some(ToolKind::Fetch) => CursorCategory::Fetch,
        Some(ToolKind::Edit | ToolKind::Delete | ToolKind::Move) => CursorCategory::Write,
        Some(ToolKind::Read) => CursorCategory::Read,
        Some(ToolKind::Search) => CursorCategory::Search,
        // Cursor does not identify MCP tools more precisely. Unknown wire
        // strings deserialize as Other; missing kinds share this best effort.
        Some(ToolKind::Other | ToolKind::Think | ToolKind::SwitchMode) | None => {
            CursorCategory::Mcp
        }
        Some(_) => return PermissionDecision::Reject,
    };
    if fence.allowed_categories.contains(&category) {
        PermissionDecision::AllowOnce
    } else {
        PermissionDecision::Reject
    }
}

#[cfg(test)]
#[path = "acp_permissions_tests.rs"]
mod tests;
