//! Antigravity's per-run fence: which built-ins exist, the agy session mode,
//! and which permission requests Rho approves.
//!
//! Three layers, all derived from the declared tools and the Rho mode:
//! 1. The run's agy custom agent lists the kept tools (see `agent_file`);
//!    agy 1.5.0 builds the agent with only those built-ins, plus
//!    `manage_task`. Plan keeps read-only tools.
//! 2. The agy session mode: `plan` in Plan, `default` in Bypass. The ACP
//!    driver fails the run if agy leaves it.
//! 3. Permission answers by request kind. agy 1.5.0 (mode `default`, Rho's
//!    own settings) asks before every command (kind `execute`); workspace
//!    edits never ask. Plan rejects everything. Bypass approves a request
//!    only when a kept tool covers its kind, so a command is refused unless
//!    `run_command` is declared, even if agy ignored the agent file.
//!
//! Auto, Allow edits, and Supervised are refused: Rho answers agy's
//! permission requests itself and cannot put a human in front of them.

use agent_client_protocol::schema::v1::{RequestPermissionRequest, ToolKind};

use crate::{
    acp_runtime::permission::PermissionDecision, agent::AntigravityTool, permission::PermissionMode,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FenceMode {
    Plan,
    Bypass,
}

/// The tools a run keeps, proven nonempty and narrowed for its mode. Only
/// [`AntigravityFence::new`] constructs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AntigravityFence {
    mode: FenceMode,
    tools: Vec<AntigravityTool>,
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
    #[error("antigravity agents in Plan need a read-only tool (read_file) in their tools list")]
    NoReadOnlyToolsInPlan,
}

impl AntigravityFence {
    /// Gate a Rho permission mode and narrow the declared tools for it: Plan
    /// keeps [`AntigravityTool::is_read_only`] tools, Bypass keeps all.
    pub(crate) fn new(
        mode: PermissionMode,
        declared: &[AntigravityTool],
    ) -> Result<Self, AntigravityFenceError> {
        let (mode, tools) = match mode {
            PermissionMode::Plan => (
                FenceMode::Plan,
                declared
                    .iter()
                    .copied()
                    .filter(|tool| tool.is_read_only())
                    .collect::<Vec<_>>(),
            ),
            PermissionMode::Bypass => (FenceMode::Bypass, declared.to_vec()),
            PermissionMode::Auto | PermissionMode::AllowEdits | PermissionMode::Supervised => {
                return Err(AntigravityFenceError::ApprovalUnsupported(mode));
            }
        };
        if tools.is_empty() {
            return Err(match mode {
                FenceMode::Plan => AntigravityFenceError::NoReadOnlyToolsInPlan,
                FenceMode::Bypass => AntigravityFenceError::NoToolsAllowed,
            });
        }
        Ok(Self { mode, tools })
    }

    pub(crate) fn tools(&self) -> &[AntigravityTool] {
        &self.tools
    }

    /// The agy session mode id.
    pub(crate) fn session_mode(&self) -> &'static str {
        match self.mode {
            FenceMode::Plan => "plan",
            FenceMode::Bypass => "default",
        }
    }

    /// Answer one permission request. Fails closed in Plan, and for any
    /// request whose kind no kept tool covers.
    pub(crate) fn decide(&self, request: &RequestPermissionRequest) -> PermissionDecision {
        match self.mode {
            FenceMode::Plan => return PermissionDecision::Reject,
            FenceMode::Bypass => {}
        }
        use AntigravityTool as T;
        let covering: &[AntigravityTool] = match request.tool_call.fields.kind {
            Some(ToolKind::Read) => &[T::ReadFile],
            Some(ToolKind::Edit | ToolKind::Delete | ToolKind::Move) => {
                &[T::WriteFile, T::EditFile]
            }
            Some(ToolKind::Execute) => &[T::RunCommand],
            Some(ToolKind::Search) => &[T::SearchWeb],
            _ => return PermissionDecision::Reject,
        };
        if covering.iter().any(|tool| self.tools.contains(tool)) {
            PermissionDecision::AllowOnce
        } else {
            PermissionDecision::Reject
        }
    }
}

#[cfg(test)]
#[path = "fence_tests.rs"]
mod tests;
