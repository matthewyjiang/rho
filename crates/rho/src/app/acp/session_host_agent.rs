//! Agent binding and restoration for host-created ACP children.

use std::{future::Future, sync::Arc};

use agent_client_protocol::schema::v1::PromptResponse;
use rho_sdk::{model::Message, UserInput};

use super::{
    build_session, commands::CommandRejected, model_scoped_config, session_config,
    session_options_for_id, split_assembly, AcpClientPort, AcpStartup, SessionHost, StoredSession,
};
use crate::{
    agent::{AgentCatalog, AgentDefinition, AgentRuntimeSpec},
    app::{
        agent_binding::{AgentBinder, AgentInvocation, AgentRole, BoundAgent, BoundRuntime},
        session_assembly::SessionAssembly,
    },
    config::Config,
};

impl SessionHost {
    /// Rebuilds a fresh child with its definition's model, prompt, and tools,
    /// then submits only the task to the model. The original command remains
    /// in display history so the host can show how the child was launched.
    pub(super) async fn run_as_agent(
        &mut self,
        display: &UserInput,
        agent_id: &str,
        task: UserInput,
        startup: &AcpStartup,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let catalog = AgentCatalog::discover(self.stored.cwd())?;
        let definition = catalog.find(agent_id)?.definition.clone();
        let cwd = self.stored.cwd().to_path_buf();
        let added_dirs = self.added_dirs.clone();
        self.run_as_agent_with(
            display,
            definition,
            task,
            startup,
            client,
            async |agent, config, id| {
                build_session(startup, &agent, &config, &cwd, &added_dirs, |_| {
                    Ok(session_options_for_id(id))
                })
                .await
            },
        )
        .await
    }

    /// Keep the rebind transaction independent of credential/provider discovery
    /// so its session and tool restrictions can use a scripted assembly in tests.
    async fn run_as_agent_with<Build, BuildFuture>(
        &mut self,
        display: &UserInput,
        definition: AgentDefinition,
        task: UserInput,
        startup: &AcpStartup,
        client: &dyn AcpClientPort,
        build: Build,
    ) -> anyhow::Result<PromptResponse>
    where
        Build: FnOnce(BoundAgent, Config, rho_sdk::SessionId) -> BuildFuture,
        BuildFuture: Future<Output = anyhow::Result<SessionAssembly>>,
    {
        // The SDK stores its initial system prompt in history; that alone is
        // not a conversation. Any other message must survive, never be dropped.
        let has_history = self
            .built
            .session
            .history()
            .iter()
            .any(|message| match message {
                Message::System(_) => false,
                Message::User(_)
                | Message::Assistant(_)
                | Message::EnrichedAssistant(_)
                | Message::AbortedAssistant(_)
                | Message::ToolResult(_) => true,
            });
        if self.completed_runs != 0 || has_history {
            return Err(CommandRejected(
                "/agent:<name> starts a fresh session only; create a new session".into(),
            )
            .into());
        }

        let bound = self.bind_delegated_agent(definition, startup)?;
        let config = match bound.runtime() {
            BoundRuntime::Rho { config, .. } => session_config(config, &self.host_mcp_servers),
            BoundRuntime::ClaudeCli { .. }
            | BoundRuntime::Cursor { .. }
            | BoundRuntime::Antigravity { .. } => {
                return Err(CommandRejected(format!(
                    "{} runs on {}, which an ACP session cannot host",
                    bound.id(),
                    bound.definition().runtime.runtime()
                ))
                .into());
            }
        };
        let id = self.built.session.id().clone();
        let assembly = build(bound.clone(), config.clone(), id).await?;
        if let Err(error) = self
            .stored
            .rebind_fresh_agent(bound.id().as_str(), &bound.fingerprint().to_string())
        {
            assembly.built.teardown().await;
            return Err(error);
        }
        let (built, permission) = split_assembly(assembly, config.permission_mode);
        let old = std::mem::replace(&mut self.built, built);
        old.teardown().await;
        self.permission = permission;
        self.auth = config.auth.clone();
        tracing::debug!(previous = %self.agent.id(), agent = %bound.id(), "rebound ACP agent");
        self.agent = bound;
        self.built.tools.bind_recall(Some(&self.stored));
        self.run_turn(display, task, Vec::new(), client).await
    }

    /// Bind against the host's current selection, not its launch defaults:
    /// T3 can select a child's model and mode before sending its first prompt.
    fn bind_delegated_agent(
        &self,
        definition: AgentDefinition,
        startup: &AcpStartup,
    ) -> anyhow::Result<BoundAgent> {
        let mut base = model_scoped_config(&self.current_model(), &startup.config);
        base.permission_mode = self.permission.mode;
        base.reasoning = self.built.session.reasoning_level();
        bind_delegated_agent(definition, startup, &base)
    }
}

/// The agent and config a loaded session runs with. ACP roots run as
/// `startup.agent`; a stored identity naming another definition was written by
/// `/agent:`, so it binds as delegated again and must match its saved
/// definition. A changed or missing definition fails the load rather than
/// widening the child to the root's tools.
/// `catalog` discovers definitions only when the session names another agent.
pub(super) fn loaded_agent(
    startup: &AcpStartup,
    stored: &StoredSession,
    config: Config,
    catalog: impl FnOnce() -> anyhow::Result<AgentCatalog>,
) -> anyhow::Result<(BoundAgent, Config)> {
    let Some((stored_id, _)) = stored.stored_agent_identity()? else {
        return Ok((startup.agent.clone(), config));
    };
    if stored_id == startup.agent.id().as_str() {
        return Ok((startup.agent.clone(), config));
    }
    let definition = catalog()?.find(&stored_id)?.definition.clone();
    stored.validate_agent_definition_identity(&definition)?;
    let bound = bind_delegated_agent(definition, startup, &config)?;
    let config = bound
        .rho_config()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("agent '{stored_id}' is not a Rho agent"))?;
    Ok((bound, config))
}

/// Both fresh rebinding and resume must enforce the delegated tool boundary.
fn bind_delegated_agent(
    definition: AgentDefinition,
    startup: &AcpStartup,
    base: &Config,
) -> anyhow::Result<BoundAgent> {
    match &definition.runtime {
        AgentRuntimeSpec::Rho { .. } => {}
        AgentRuntimeSpec::ClaudeCli(_)
        | AgentRuntimeSpec::Cursor(_)
        | AgentRuntimeSpec::Antigravity(_) => {
            return Err(CommandRejected(format!(
                "{} runs on {}, which an ACP session cannot host",
                definition.id,
                definition.runtime.runtime()
            ))
            .into());
        }
    }
    AgentBinder::bind(
        Arc::new(definition),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: startup
                .agent
                .rho_capabilities()
                .cloned()
                .unwrap_or_default(),
        },
        base,
    )
}

#[cfg(test)]
#[path = "session_host_agent_tests.rs"]
mod tests;
