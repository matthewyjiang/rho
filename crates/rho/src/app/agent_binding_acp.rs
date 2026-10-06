//! Bind ACP subagent runtimes (Cursor, Antigravity).
//!
//! Both are delegated-only, take a pass-through model string, and support
//! only Plan and Bypass, which bind checks so launch never reaches spawn.

use crate::{
    agent::{
        AgentDefinition, AgentRuntime, AntigravityAgentConfig, AntigravityTool, CursorAgentConfig,
        CursorTool,
    },
    config::Config,
    permission::PermissionMode,
    workflow::ResolvedAgent,
};

use super::{AgentInvocation, AgentRole, BoundRuntime};

pub(super) fn bind_cursor_runtime(
    definition: &AgentDefinition,
    config: &CursorAgentConfig,
    invocation: &AgentInvocation,
    host_config: &Config,
) -> anyhow::Result<BoundRuntime> {
    check_delegated_pass_through(
        definition,
        AgentRuntime::Cursor,
        config.model.as_deref(),
        invocation,
        "gpt-5.3-codex",
    )?;
    crate::cursor_runtime::spawn::map_permission_mode(host_config.permission_mode, &config.tools)
        .map_err(|error| anyhow::anyhow!("agent '{}': {error}", definition.id))?;
    Ok(BoundRuntime::Cursor {
        model: config.model.clone(),
        tools: config.tools.clone(),
        permission_mode: host_config.permission_mode,
    })
}

pub(super) fn bind_antigravity_runtime(
    definition: &AgentDefinition,
    config: &AntigravityAgentConfig,
    invocation: &AgentInvocation,
    host_config: &Config,
) -> anyhow::Result<BoundRuntime> {
    check_delegated_pass_through(
        definition,
        AgentRuntime::Antigravity,
        config.model.as_deref(),
        invocation,
        "gemini-3.8-flash-high",
    )?;
    crate::antigravity_runtime::fence::map_permission_mode(
        host_config.permission_mode,
        &config.tools,
    )
    .map_err(|error| anyhow::anyhow!("agent '{}': {error}", definition.id))?;
    Ok(BoundRuntime::Antigravity {
        model: config.model.clone(),
        tools: config.tools.clone(),
        permission_mode: host_config.permission_mode,
    })
}

/// Delegated-only role, and no Rho `@alias` model. Parse already rejects
/// aliases; constructed configs (tests, future loaders) must not slip past.
fn check_delegated_pass_through(
    definition: &AgentDefinition,
    runtime: AgentRuntime,
    model: Option<&str>,
    invocation: &AgentInvocation,
    model_example: &str,
) -> anyhow::Result<()> {
    match invocation.role {
        AgentRole::Delegated | AgentRole::Workflow => {}
        AgentRole::InteractiveRoot | AgentRole::AutomationRoot => {
            anyhow::bail!(
                "agent '{}': runtime {runtime} is delegated-only; use it through the agent tool, not as an interactive or automation root",
                definition.id
            );
        }
    }
    if let Some(model) = model.filter(|model| model.starts_with('@')) {
        anyhow::bail!(
            "agent '{}': runtime {runtime} does not resolve Rho model aliases; \
set a {runtime} model name (for example {model_example}), not '{model}'",
            definition.id
        );
    }
    Ok(())
}

pub(super) fn frozen_cursor_runtime(
    frozen: &ResolvedAgent,
    permission_mode: PermissionMode,
) -> anyhow::Result<BoundRuntime> {
    let tools = frozen_tools::<CursorTool>(frozen)?;
    crate::cursor_runtime::spawn::map_permission_mode(permission_mode, &tools)
        .map_err(|error| anyhow::anyhow!("agent '{}': {error}", frozen.agent_id))?;
    Ok(BoundRuntime::Cursor {
        model: frozen.model.clone(),
        tools,
        permission_mode,
    })
}

pub(super) fn frozen_antigravity_runtime(
    frozen: &ResolvedAgent,
    permission_mode: PermissionMode,
) -> anyhow::Result<BoundRuntime> {
    let tools = frozen_tools::<AntigravityTool>(frozen)?;
    crate::antigravity_runtime::fence::map_permission_mode(permission_mode, &tools)
        .map_err(|error| anyhow::anyhow!("agent '{}': {error}", frozen.agent_id))?;
    Ok(BoundRuntime::Antigravity {
        model: frozen.model.clone(),
        tools,
        permission_mode,
    })
}

/// A frozen plan's capability names in a closed tool vocabulary.
fn frozen_tools<T>(frozen: &ResolvedAgent) -> anyhow::Result<Vec<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    frozen
        .capabilities
        .iter()
        .map(|name| {
            name.parse::<T>()
                .map_err(|error| anyhow::anyhow!("frozen agent '{}': {error}", frozen.agent_id))
        })
        .collect()
}
