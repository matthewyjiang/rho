//! The launch for a bound agent: a Rho child, or the session request for a
//! CLI runtime (Claude Code, Cursor, Antigravity) built from the bound agent
//! plus what one run launches with, including the environment it runs in
//! (#1659).

use super::{BoundAgent, BoundRuntime};

/// What one launch of a bound agent starts with besides the agent itself.
/// A CLI runtime's session request takes all of it; a Rho agent ignores it,
/// since its child run takes the same values from its launch request.
pub(crate) struct CliRunInputs {
    pub(crate) prompt: String,
    pub(crate) output_file: std::path::PathBuf,
    pub(crate) cwd: std::path::PathBuf,
    pub(crate) cancellation: rho_tools::cancellation::RunCancellation,
    pub(crate) status_tx: Option<tokio::sync::watch::Sender<crate::subagent::RunStatus>>,
    pub(crate) started_status: Option<crate::subagent::RunStatus>,
    /// The environment a CLI agent runs in and is found on: its session's
    /// when the run launched (#1659).
    pub(crate) environment: crate::session_environment::Ambient,
}

/// Exhaustive launch target after bind. The session task matches this instead
/// of probing Claude-then-Rho.
pub(in crate::app) enum Launch {
    Rho(BoundAgent),
    ClaudeCli(crate::claude_runtime::session::ClaudeSessionRequest),
    Cursor(crate::cursor_runtime::session::CursorSessionRequest),
    Antigravity(crate::antigravity_runtime::session::AntigravitySessionRequest),
}

impl BoundAgent {
    /// The launch for this agent: a CLI runtime's session request built
    /// from `run`, or the agent itself for a Rho child, which leaves `run`
    /// unused.
    pub(in crate::app) fn into_launch(self, run: CliRunInputs) -> Launch {
        let CliRunInputs {
            prompt,
            output_file,
            cwd,
            cancellation,
            status_tx,
            started_status,
            environment,
        } = run;
        let identity = self.artifact_identity();
        match self.runtime {
            BoundRuntime::Rho { .. } => Launch::Rho(self),
            BoundRuntime::ClaudeCli {
                tools,
                inherit_claude_config,
                permission_mode,
                max_turns,
                // Both are already in `identity`.
                model: _,
                reasoning: _,
            } => Launch::ClaudeCli(crate::claude_runtime::session::ClaudeSessionRequest {
                system_prompt: self.definition.prompt.clone(),
                identity,
                tools,
                inherit_claude_config,
                permission_mode,
                max_turns,
                prompt,
                output_file,
                cwd,
                cancellation,
                status_tx,
                started_status,
                parent_messages: None,
                rate_limit_state_path: None,
                overrides: Default::default(),
                // A saved parent's; the executor sets the parent's own.
                persistence: crate::session::SessionPersistence::Saved,
                environment,
            }),
            BoundRuntime::Cursor {
                tools,
                permission_mode,
                // Already in `identity`.
                model: _,
            } => Launch::Cursor(crate::cursor_runtime::session::CursorSessionRequest {
                system_prompt: self.definition.prompt.clone(),
                identity,
                tools,
                prompt,
                output_file,
                cwd,
                permission_mode,
                cancellation,
                status_tx,
                started_status,
                parent_messages: None,
                // No home dir leaves only `CURSOR_CONFIG_DIR` / XDG to find the
                // user's config; a missing file derives the managed config from `{}`.
                config_paths: crate::cursor_runtime::acp_config::CursorConfigPaths::from_env(
                    &environment,
                    crate::paths::home_dir().unwrap_or_default(),
                ),
                overrides: Default::default(),
                environment,
            }),
            BoundRuntime::Antigravity {
                tools,
                permission_mode,
                // Already in `identity`.
                model: _,
            } => Launch::Antigravity(
                crate::antigravity_runtime::session::AntigravitySessionRequest {
                    system_prompt: self.definition.prompt.clone(),
                    identity,
                    tools,
                    prompt,
                    output_file,
                    cwd,
                    permission_mode,
                    cancellation,
                    status_tx,
                    started_status,
                    parent_messages: None,
                    overrides: Default::default(),
                    environment,
                },
            ),
        }
    }
}
