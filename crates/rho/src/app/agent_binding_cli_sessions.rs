//! Session requests for a bound CLI runtime (Claude Code, Cursor,
//! Antigravity): the bound agent plus what one run launches with, including
//! the environment it runs in (#1659).

use super::{BoundAgent, BoundRuntime};

/// What a CLI runtime run (Claude Code, Cursor, Antigravity) launches with
/// besides its bound agent.
pub(crate) struct CliRunInputs {
    pub(crate) prompt: String,
    pub(crate) output_file: std::path::PathBuf,
    pub(crate) cwd: std::path::PathBuf,
    pub(crate) cancellation: rho_tools::cancellation::RunCancellation,
    pub(crate) status_tx: Option<tokio::sync::watch::Sender<crate::subagent::RunStatus>>,
    pub(crate) started_status: Option<crate::subagent::RunStatus>,
    /// The environment the agent runs in and is found on: its session's when
    /// the run launched (#1659).
    pub(crate) environment: crate::session_environment::Ambient,
}

impl BoundAgent {
    /// Build the Claude session request for a bound Claude runtime.
    pub(crate) fn into_claude_session(
        self,
        run: CliRunInputs,
    ) -> Option<crate::claude_runtime::session::ClaudeSessionRequest> {
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
        let BoundRuntime::ClaudeCli {
            tools,
            inherit_claude_config,
            permission_mode,
            max_turns,
            ..
        } = self.runtime
        else {
            return None;
        };
        Some(crate::claude_runtime::session::ClaudeSessionRequest {
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
        })
    }

    /// Build the Cursor session request for a bound Cursor runtime.
    pub(crate) fn into_cursor_session(
        self,
        run: CliRunInputs,
    ) -> Option<crate::cursor_runtime::session::CursorSessionRequest> {
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
        let BoundRuntime::Cursor {
            tools,
            permission_mode,
            ..
        } = self.runtime
        else {
            return None;
        };
        Some(crate::cursor_runtime::session::CursorSessionRequest {
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
        })
    }

    /// Build the Antigravity session request for a bound Antigravity runtime.
    pub(crate) fn into_antigravity_session(
        self,
        run: CliRunInputs,
    ) -> Option<crate::antigravity_runtime::session::AntigravitySessionRequest> {
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
        let BoundRuntime::Antigravity {
            tools,
            permission_mode,
            ..
        } = self.runtime
        else {
            return None;
        };
        Some(
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
        )
    }
}
