//! Build argv for `cursor-agent acp` subagent runs and gate permission modes.
//!
//! Under `acp`, `--allowed-tools` and `--mode` are ignored (spike 2026-10-05,
//! `acp_runtime/fixtures/README.md`), so argv only carries identity. Tool
//! fencing lives in the Rho-managed Cursor config (`acp_config`) and plan mode
//! is set with `session/set_mode`.

use std::path::{Path, PathBuf};

use crate::{agent::CursorTool, permission::PermissionMode};

/// The tool list a run may keep, proven nonempty and already narrowed for its
/// permission mode (Plan keeps read-only tools).
///
/// Only [`map_permission_mode`] constructs this, so every fence passes through
/// that gate: an empty list has no meaningful fence, so nonemptiness must be
/// unforgeable rather than a convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AllowedTools {
    tools: Vec<CursorTool>,
}

impl AllowedTools {
    pub(crate) fn tools(&self) -> &[CursorTool] {
        &self.tools
    }
}

/// Inputs needed to construct a Cursor CLI spawn.
///
/// Model comes from the bound runtime contract, not from re-interpreting
/// parent provider/model config.
#[derive(Clone, Debug)]
pub(crate) struct CursorSpawnRequest {
    /// Cursor `--model` value. `None` means omit the flag (Cursor inherit).
    pub(crate) model: Option<String>,
    pub(crate) cwd: PathBuf,
}

/// The full spawn contract: argv is the only carrier of flag decisions, so
/// tests and production read the same values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorSpawnPlan {
    /// Global flags followed by the `acp` subcommand, always last.
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CursorSpawnError {
    #[error(
        "cursor agents run only in Plan or Bypass, not {0}: Rho cannot ask a human to approve Cursor tool calls"
    )]
    ApprovalUnsupported(PermissionMode),
    #[error(
        "cursor agents require a nonempty tools list: it is the source of Cursor's tool fence"
    )]
    NoToolsAllowed,
}

/// Gate a Rho permission mode for Cursor and narrow the tools it may keep.
///
/// Plan intersects the declared list with [`CursorTool::is_read_only`].
/// Bypass keeps every declared tool. Auto, Allow edits, and Supervised are
/// refused: Rho answers Cursor's permission requests and cannot prompt.
pub(crate) fn map_permission_mode(
    mode: PermissionMode,
    tools: &[CursorTool],
) -> Result<AllowedTools, CursorSpawnError> {
    let tools: Vec<CursorTool> = match mode {
        PermissionMode::Plan => tools
            .iter()
            .copied()
            .filter(|tool| tool.is_read_only())
            .collect(),
        PermissionMode::Bypass => tools.to_vec(),
        PermissionMode::Auto | PermissionMode::AllowEdits | PermissionMode::Supervised => {
            return Err(CursorSpawnError::ApprovalUnsupported(mode));
        }
    };
    if tools.is_empty() {
        return Err(CursorSpawnError::NoToolsAllowed);
    }
    Ok(AllowedTools { tools })
}

/// Identity flags that a frozen workflow argv may keep.
///
/// Permission-sensitive flags never come from frozen argv; under `acp` they
/// are ignored anyway, and the fence is regenerated from the bound mode.
const FROZEN_IDENTITY_FLAGS: &[&str] = &["--model"];

/// The ACP subcommand. Global flags must precede it.
const ACP_SUBCOMMAND: &str = "acp";

/// Overlay frozen identity onto a freshly generated plan, keeping `acp` last.
pub(crate) fn apply_frozen_identity_args(
    mut plan: CursorSpawnPlan,
    frozen: &[String],
) -> CursorSpawnPlan {
    let subcommand = plan.args.pop();
    debug_assert_eq!(subcommand.as_deref(), Some(ACP_SUBCOMMAND));
    plan.args =
        crate::cli_runtime::overlay_identity_flags(plan.args, frozen, FROZEN_IDENTITY_FLAGS);
    plan.args.push(ACP_SUBCOMMAND.into());
    plan
}

/// Build argv for a Cursor ACP spawn: `--trust` (no workspace prompt on
/// stdio), optional `--model`, then `acp`.
pub(crate) fn build_spawn_plan(request: &CursorSpawnRequest) -> CursorSpawnPlan {
    let mut args = vec!["--trust".to_string()];
    if let Some(model) = &request.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    args.push(ACP_SUBCOMMAND.into());
    CursorSpawnPlan {
        args,
        cwd: request.cwd.clone(),
    }
}

pub(crate) fn log_path(output_file: &Path) -> PathBuf {
    output_file.with_file_name(crate::subagent::LOG_FILE_NAME)
}

#[cfg(test)]
#[path = "spawn_tests.rs"]
mod tests;
