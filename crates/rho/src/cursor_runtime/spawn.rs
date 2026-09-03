//! Build argv for `cursor-agent -p` subagent runs.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{
    agent::{CursorTool, PromptPolicy},
    permission::PermissionMode,
};

/// Cursor CLI permission class Rho will set on `cursor-agent -p`.
///
/// This is not Rho's [`PermissionMode`]. Cursor has no approval protocol in
/// `-p`; only plan mode and an explicit allow list restrict the child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorPermissionMode {
    /// `--mode plan` plus a read-only allow list.
    Plan,
    /// No `--mode`; tools run at full power inside the allow list.
    Full,
}

/// The allow list a spawn may pass, proven nonempty and already narrowed for
/// its permission class.
///
/// Only [`map_permission_mode`] constructs this, so every `--allowed-tools`
/// argv passes through that gate: `cursor-agent -p` is full-power by default
/// and an empty list has unverified semantics, so nonemptiness must be
/// unforgeable rather than a convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AllowedTools {
    mode: CursorPermissionMode,
    tools: Vec<CursorTool>,
}

impl AllowedTools {
    pub(crate) fn mode(&self) -> CursorPermissionMode {
        self.mode
    }

    pub(crate) fn tools(&self) -> &[CursorTool] {
        &self.tools
    }
}

/// Inputs needed to construct a Cursor CLI spawn.
///
/// Model and tools come from the bound runtime contract, not from
/// re-interpreting parent provider/model config.
#[derive(Clone, Debug)]
pub(crate) struct CursorSpawnRequest {
    /// Cursor `--model` value. `None` means omit the flag (Cursor inherit).
    pub(crate) model: Option<String>,
    /// Permission class plus the tools it may keep.
    pub(crate) allowed: AllowedTools,
    pub(crate) cwd: PathBuf,
}

/// The full spawn contract: argv is the only carrier of flag decisions, so
/// tests and production read the same values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorSpawnPlan {
    /// Argv without `--new-session-id`. Call [`finalize_spawn_args`] to attach it.
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CursorSpawnError {
    #[error(
        "cursor agents run only in Plan or Bypass, not {0}: `cursor-agent -p` has no approval protocol and `--exclude-tools` does not fence"
    )]
    ApprovalUnsupported(PermissionMode),
    #[error(
        "cursor agents require a nonempty --allowed-tools list: `cursor-agent -p` enables every tool by default"
    )]
    NoToolsAllowed,
}

/// Failures while composing the stdin prompt.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CursorPromptError {
    #[error(
        "cursor: internal error: PromptPolicy::Replace cannot reach spawn (parser rejects it)"
    )]
    ReplaceUnsupported,
}

/// Map Rho permission mode onto a Cursor CLI class and the tools that class
/// may keep.
///
/// Plan intersects the declared list with [`CursorTool::is_read_only`].
/// Bypass keeps every declared tool. Auto, Allow edits, and Supervised are
/// refused: `-p` cannot prompt and `--exclude-tools` does not fence.
pub(crate) fn map_permission_mode(
    mode: PermissionMode,
    tools: &[CursorTool],
) -> Result<AllowedTools, CursorSpawnError> {
    let (mode, tools) = match mode {
        PermissionMode::Plan => (
            CursorPermissionMode::Plan,
            tools
                .iter()
                .copied()
                .filter(|tool| tool.is_read_only())
                .collect(),
        ),
        PermissionMode::Bypass => (CursorPermissionMode::Full, tools.to_vec()),
        PermissionMode::Auto | PermissionMode::AllowEdits | PermissionMode::Supervised => {
            return Err(CursorSpawnError::ApprovalUnsupported(mode));
        }
    };
    if tools.is_empty() {
        return Err(CursorSpawnError::NoToolsAllowed);
    }
    Ok(AllowedTools { mode, tools })
}

/// Identity flags that a frozen workflow argv may keep.
///
/// Permission-sensitive flags (`--mode`, `--allowed-tools`, and any force /
/// exclude / thinking switch) are always taken from the regenerated plan.
const FROZEN_IDENTITY_FLAGS: &[&str] = &["--model"];

/// Overlay frozen identity onto argv generated from the effective bound mode.
///
/// Frozen permission flags cannot widen or replace the mapped Cursor mode.
pub(crate) fn apply_frozen_identity_args(generated: Vec<String>, frozen: &[String]) -> Vec<String> {
    crate::cli_runtime::overlay_identity_flags(generated, frozen, FROZEN_IDENTITY_FLAGS)
}

/// Build argv for a Cursor spawn. Infallible once the request carries a
/// resolved [`CursorPermissionMode`] and a nonempty allow list.
pub(crate) fn build_spawn_plan(request: &CursorSpawnRequest) -> CursorSpawnPlan {
    let mut args = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--stream-partial-output".into(),
        "--trust".into(),
    ];
    if let Some(model) = &request.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    if matches!(request.allowed.mode(), CursorPermissionMode::Plan) {
        args.push("--mode".into());
        args.push("plan".into());
    }
    // Always set `--allowed-tools`. Default `-p` is full-power; the list is
    // nonempty by construction of [`AllowedTools`].
    args.push("--allowed-tools".into());
    args.push(
        request
            .allowed
            .tools()
            .iter()
            .map(|tool| tool.as_flag())
            .collect::<Vec<_>>()
            .join(","),
    );

    CursorSpawnPlan {
        args,
        cwd: request.cwd.clone(),
    }
}

/// Append `--new-session-id`. Mutually exclusive with `--resume`; v1 never resumes.
pub(crate) fn finalize_spawn_args(plan: &CursorSpawnPlan, session_id: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = plan.args.iter().map(OsString::from).collect();
    args.push(OsString::from("--new-session-id"));
    args.push(OsString::from(session_id));
    args
}

/// Apply [`PromptPolicy`] to the user turn that will be written to stdin.
///
/// `--system-prompt` is rejected server-side, so Extend text is prepended to
/// the user prompt. Empty Extend sends the prompt alone. Replace cannot reach
/// here (parser rejects it) and is an internal error.
pub(crate) fn compose_prompt(
    system_prompt: &PromptPolicy,
    user_prompt: &str,
) -> Result<String, CursorPromptError> {
    match system_prompt {
        PromptPolicy::Extend(text) if text.is_empty() => Ok(user_prompt.to_string()),
        PromptPolicy::Extend(text) => Ok(format!("{text}\n\n---\n\n{user_prompt}")),
        PromptPolicy::Replace(_) => Err(CursorPromptError::ReplaceUnsupported),
    }
}

pub(crate) fn log_path(output_file: &Path) -> PathBuf {
    output_file.with_file_name(crate::subagent::LOG_FILE_NAME)
}

#[cfg(test)]
#[path = "spawn_tests.rs"]
mod tests;
