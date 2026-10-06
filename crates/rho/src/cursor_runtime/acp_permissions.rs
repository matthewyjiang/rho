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
        // cursor-agent 2026.10.01 (`formatOperation`) asks for MCP tools with
        // kind `other` and a `<server>: <tool>` title; anything it cannot
        // describe is kind `other` titled "Unknown operation". Unknown wire
        // kinds also deserialize as Other. Only that MCP shape counts as MCP;
        // Think, SwitchMode, and a missing kind never do, and fail closed.
        Some(ToolKind::Other)
            if request
                .tool_call
                .fields
                .title
                .as_deref()
                .is_some_and(is_mcp_title) =>
        {
            CursorCategory::Mcp
        }
        _ => return PermissionDecision::Reject,
    };
    if fence.allowed_categories.contains(&category) {
        PermissionDecision::AllowOnce
    } else {
        PermissionDecision::Reject
    }
}

/// `<server>: <tool>`, both non-empty.
fn is_mcp_title(title: &str) -> bool {
    title
        .split_once(": ")
        .is_some_and(|(server, tool)| !server.trim().is_empty() && !tool.trim().is_empty())
}

#[cfg(test)]
#[path = "acp_permissions_tests.rs"]
mod tests;
