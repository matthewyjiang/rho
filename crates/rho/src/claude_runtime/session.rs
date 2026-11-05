//! Execute a `runtime: claude-cli` delegated run via `claude-agent-acp`.

use std::path::PathBuf;

use tokio::sync::watch;

use rho_tools::cancellation::RunCancellation;

use crate::{
    acp_runtime::{self, AcpSessionRequest},
    agent::PromptPolicy,
    cli_runtime::{
        parent_messages::ParentMessageInbox, status_sink::RuntimeLabel, CliSessionOverrides,
    },
    permission::PermissionMode,
    run_artifacts::RunArtifactIdentity,
    subagent::RunStatus,
};

use super::policy::{ClaudeAcpPolicy, ClaudeSessionShape, SessionPersistence};

/// Claude Code labels for the shared artifact sink. ACP session ids are
/// Claude session ids, so the bundled CLI resumes them.
pub(crate) const CLAUDE_LABEL: RuntimeLabel = RuntimeLabel {
    starting_activity: "starting claude",
    program: "claude code",
    resume_command: Some("claude-agent-acp --cli"),
    session_label: "claude session",
    cost_label: "claude cost",
};

/// Inputs for one Claude subagent run, including bound runtime values.
pub(crate) struct ClaudeSessionRequest {
    /// Carried as `_meta.systemPrompt`: Replace replaces Claude's system
    /// prompt and Extend appends to it.
    pub(crate) system_prompt: PromptPolicy,
    /// Bound snapshot stamped onto `result.json`. Model and reasoning come
    /// from here so they cannot drift from the Starting identity.
    pub(crate) identity: RunArtifactIdentity,
    pub(crate) tools: Vec<String>,
    pub(crate) inherit_claude_config: bool,
    /// Exact Claude turn cap, set from the step limit at bind time.
    pub(crate) max_turns: u64,
    pub(crate) prompt: String,
    pub(crate) output_file: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    /// Starting status the launcher already wrote to `result.json`.
    pub(crate) started_status: Option<RunStatus>,
    /// Steered into the running turn, or sent as the next one.
    pub(crate) parent_messages: Option<ParentMessageInbox>,
    /// Rate-limit cache path. Tests inject a temp path so runs never touch
    /// the host cache.
    pub(crate) rate_limit_state_path: Option<PathBuf>,
    pub(crate) overrides: CliSessionOverrides,
}

/// Run one Claude session to completion, writing the subagent contract.
pub(crate) async fn run_session(request: ClaudeSessionRequest) -> anyhow::Result<()> {
    let ClaudeSessionRequest {
        system_prompt,
        identity,
        tools,
        inherit_claude_config,
        max_turns,
        prompt,
        output_file,
        cwd,
        permission_mode,
        cancellation,
        status_tx,
        started_status,
        parent_messages,
        rate_limit_state_path,
        overrides,
    } = request;
    let policy = ClaudeAcpPolicy::delegated(
        ClaudeSessionShape {
            system_prompt,
            model: identity.model.clone(),
            reasoning: identity.reasoning,
            tools,
            max_turns,
            persistence: SessionPersistence::Keep,
        },
        permission_mode,
        inherit_claude_config,
        cwd.clone(),
        rate_limit_state_path,
    );
    let request = AcpSessionRequest {
        identity,
        prompt,
        // The policy sends the system prompt in `_meta`; nothing to prepend.
        system_prompt: PromptPolicy::Extend(String::new()),
        output_file,
        cwd,
        cancellation,
        status_tx,
        started_status,
        parent_messages,
        overrides,
    };
    acp_runtime::run_session(request, policy).await
}
