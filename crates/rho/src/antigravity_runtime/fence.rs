//! Antigravity's per-run fence: which built-ins exist, and which permission
//! requests Rho approves.
//!
//! Two layers, both derived from the declared tools and the Rho mode:
//! 1. `_meta.agy.enabledTools` on `session/new` is the per-tool allowlist.
//!    Undeclared built-ins never reach the model (verified: `[view_file]`
//!    leaves only `view_file`).
//! 2. Permission answers gate by category. agy_acp_server 1.3.0 asks (in mode
//!    `default`) with kind `edit` for create/edit, `execute` for
//!    `run_command`, `search` for `search_web`, and `fetch` for
//!    `read_url_content`; `view_file` never asks. Anything else rejects.

use agent_client_protocol::schema::v1::{Meta, RequestPermissionRequest, ToolKind};
use serde_json::json;

use crate::{
    acp_runtime::permission::PermissionDecision, agent::AntigravityTool, permission::PermissionMode,
};

/// The tools a run keeps, proven nonempty and narrowed for its mode (Plan
/// keeps read-only tools). Only [`map_permission_mode`] constructs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AllowedTools {
    tools: Vec<AntigravityTool>,
}

impl AllowedTools {
    pub(crate) fn tools(&self) -> &[AntigravityTool] {
        &self.tools
    }

    /// `session/new` `_meta`: the built-in allowlist.
    pub(crate) fn session_meta(&self) -> Meta {
        let names = self
            .tools
            .iter()
            .map(|tool| tool.as_name())
            .collect::<Vec<_>>();
        let mut meta = Meta::new();
        meta.insert("agy".into(), json!({ "enabledTools": names }));
        meta
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AntigravityFenceError {
    #[error(
        "antigravity agents run only in Plan or Bypass, not {0}: Rho cannot ask a human to approve Antigravity tool calls"
    )]
    ApprovalUnsupported(PermissionMode),
    #[error(
        "antigravity agents require a nonempty tools list: it is the source of Antigravity's tool allowlist"
    )]
    NoToolsAllowed,
    #[error("antigravity agents in Plan need a read-only tool (view_file) in their tools list")]
    NoReadOnlyToolsInPlan,
}

/// Gate a Rho permission mode for Antigravity and narrow the tools it keeps.
///
/// Plan keeps [`AntigravityTool::is_read_only`] tools. Bypass keeps every
/// declared tool. Auto, Allow edits, and Supervised are refused: Rho answers
/// Antigravity's permission requests and cannot prompt.
pub(crate) fn map_permission_mode(
    mode: PermissionMode,
    tools: &[AntigravityTool],
) -> Result<AllowedTools, AntigravityFenceError> {
    let tools: Vec<AntigravityTool> = match mode {
        PermissionMode::Plan => tools
            .iter()
            .copied()
            .filter(|tool| tool.is_read_only())
            .collect(),
        PermissionMode::Bypass => tools.to_vec(),
        PermissionMode::Auto | PermissionMode::AllowEdits | PermissionMode::Supervised => {
            return Err(AntigravityFenceError::ApprovalUnsupported(mode));
        }
    };
    if tools.is_empty() {
        return Err(match mode {
            PermissionMode::Plan => AntigravityFenceError::NoReadOnlyToolsInPlan,
            PermissionMode::Bypass
            | PermissionMode::Auto
            | PermissionMode::AllowEdits
            | PermissionMode::Supervised => AntigravityFenceError::NoToolsAllowed,
        });
    }
    Ok(AllowedTools { tools })
}

/// Answer one permission request. Fails closed outside Bypass, and for any
/// request whose kind is not a declared category.
pub(crate) fn decide(
    allowed: &AllowedTools,
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
    use AntigravityTool as T;
    // Local search built-ins are outside the vocabulary, so a `search`
    // request is web search. MCP tools are unsupported in this runtime.
    let category: &[AntigravityTool] = match request.tool_call.fields.kind {
        Some(ToolKind::Read) => &[T::ViewFile],
        Some(ToolKind::Edit | ToolKind::Delete | ToolKind::Move) => &[T::CreateFile, T::EditFile],
        Some(ToolKind::Execute) => &[T::RunCommand],
        Some(ToolKind::Search) => &[T::SearchWeb],
        Some(ToolKind::Fetch) => &[T::ReadUrlContent],
        _ => return PermissionDecision::Reject,
    };
    if category.iter().any(|tool| allowed.tools.contains(tool)) {
        PermissionDecision::AllowOnce
    } else {
        PermissionDecision::Reject
    }
}

#[cfg(test)]
#[path = "fence_tests.rs"]
mod tests;
