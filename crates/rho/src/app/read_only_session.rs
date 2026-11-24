//! Sessions that read the workspace and change nothing: `/side`'s aside and
//! the title agent's run.
//!
//! Each runs a Rho agent bound to [`read_only_tools`] in plan mode, with no
//! MCP servers, no subagents, and nobody to answer an approval, so a read
//! the workspace policy would ask about is denied instead.

use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

use rho_sdk::{model::Message, SessionId, SessionOptions};

use super::{
    agent_binding::{AgentInvocation, AgentRole, BoundAgent},
    session_assembly::{
        assemble_session, ApprovalInputs, BuiltSession, HookStart, ModelOwner, SessionApproval,
        SessionAssemblyOptions, SignIn,
    },
    tools_prompt::{ToolSourceAttach, ToolSourceLoad},
};
use crate::{
    added_dirs::AddedDirs,
    agent::{read_only_tools, AgentCapabilities},
    diagnostics::RuntimeDiagnostics,
    permission::PermissionMode,
    session_environment::SessionEnvironment,
    tools::mcp::{config::McpConfig, McpAuthorizationMode, McpElicitationSupport},
};

/// What one read-only session is built from.
pub(super) struct ReadOnlySession<'a> {
    /// A Rho agent bound with [`invocation`], whose bound config is the
    /// session's: its model, auth, and reasoning.
    pub(super) agent: &'a BoundAgent,
    pub(super) config_path: PathBuf,
    pub(super) cwd: &'a Path,
    /// Directories the session may read beside `cwd`.
    pub(super) added_dirs: &'a AddedDirs,
    /// Whose credentials the session's model signs in with.
    pub(super) environment: SessionEnvironment,
    /// The most model steps one run may take.
    pub(super) max_steps: NonZeroUsize,
    pub(super) usage_purpose: &'static str,
    /// The session this one works for, which its usage names.
    pub(super) parent: SessionId,
    pub(super) hook_start: HookStart,
    pub(super) model_owner: ModelOwner,
    /// What the session's history starts with.
    pub(super) history: Vec<Message>,
}

/// How a read-only session's agent is bound: delegated, on the read-only
/// tools alone.
pub(super) fn invocation() -> AgentInvocation {
    AgentInvocation {
        role: AgentRole::Delegated,
        available_tools: AgentCapabilities::new(read_only_tools()),
    }
}

/// Builds `session`. Its owner runs it and tears it down.
pub(super) async fn assemble(session: ReadOnlySession<'_>) -> anyhow::Result<BuiltSession> {
    let ReadOnlySession {
        agent,
        config_path,
        cwd,
        added_dirs,
        environment,
        max_steps,
        usage_purpose,
        parent,
        hook_start,
        model_owner,
        history,
    } = session;
    let mut config = agent
        .rho_config()
        .ok_or_else(|| anyhow::anyhow!("agent {} does not run on rho", agent.id()))?
        .clone();
    config.mcp = McpConfig::default();
    config.permission_mode = PermissionMode::Plan;
    let diagnostics = RuntimeDiagnostics::new(&config);
    let assembled = assemble_session(SessionAssemblyOptions {
        config: &config,
        config_path,
        cwd,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: true,
        questionnaire_enabled: false,
        surfaces: None,
        pull_requests: None,
        mcp_elicitation: McpElicitationSupport::Unavailable,
        tool_source_attach: ToolSourceAttach::None,
        tool_source_load: ToolSourceLoad::Inline,
        hook_start,
        sign_in: SignIn::Required,
        model_owner,
        mcp_authorization: McpAuthorizationMode::NonInteractive,
        diagnostics: &diagnostics,
        agent,
        max_steps: Some(max_steps),
        usage_purpose,
        usage_parent_session_id: Some(parent),
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        added_dirs,
        environment,
        subagent_pool: crate::app::subagent_pool::SubagentPool::Own,
        // A read-only session starts no process it would idle on.
        process_keep_warm: crate::tools::process::KeepWarmOffer::Hidden,
        extend_tools: std::convert::identity,
        system_prompt_suffix: None,
        approval: |_: ApprovalInputs| {
            Ok(SessionApproval {
                session: None,
                receiver: None,
            })
        },
        session_options: move |_| Ok(SessionOptions::new().id(SessionId::new()).history(history)),
    })
    .await?;
    Ok(assembled.built)
}
