//! Agent binding and restoration for host-created ACP children.

use std::{future::Future, sync::Arc};

use agent_client_protocol::schema::v1::PromptResponse;
use rho_sdk::{model::Message, SessionOptions, UserInput};
use serde::{Deserialize, Serialize};

use super::{
    build_session,
    commands::{AgentTask, CommandRejected},
    model_scoped_config, session_config, session_options_for_id, split_assembly, AcpClientPort,
    AcpStartup, SessionHost, StoredSession,
};
use crate::{
    agent::{AgentCatalog, AgentDefinition, AgentRuntimeSpec},
    app::{
        agent_binding::{
            AgentBinder, AgentInvocation, AgentRole, BoundAgent, BoundRuntime, ModelOverride,
        },
        session_assembly::SessionAssembly,
    },
    config::Config,
};

/// Resolved selection, not the alias or definition: alias edits must not move
/// a resumed call to another provider. Auth is a profile id, never a credential.
const MODEL_OVERRIDE_METADATA: &str = "rho.acp.agent_model_override";

#[derive(Deserialize, Serialize)]
struct SavedModelOverride {
    provider: String,
    model: String,
    auth: String,
}

/// A later host-selected model supersedes the launch override. Keep the marker
/// and concrete selection together at every durable save, including step saves.
pub(super) fn decorate_model_override(
    snapshot: rho_sdk::SessionSnapshot,
    auth: &str,
) -> rho_sdk::SessionSnapshot {
    if !snapshot.metadata().contains_key(MODEL_OVERRIDE_METADATA) {
        return snapshot;
    }
    let identity = snapshot.provider();
    let saved = SavedModelOverride {
        provider: identity.provider.clone(),
        model: identity.model.clone(),
        auth: auth.to_owned(),
    };
    snapshot.with_metadata(
        MODEL_OVERRIDE_METADATA,
        serde_json::to_string(&saved).expect("model selection contains only strings"),
    )
}

impl SessionHost {
    /// A host-selected replacement must be bindable on the next load. Validate
    /// through the same definition policy before touching live or saved state.
    pub(super) fn validate_model_override_switch(&self, config: &Config) -> anyhow::Result<()> {
        if !self
            .built
            .session
            .snapshot()
            .metadata()
            .contains_key(MODEL_OVERRIDE_METADATA)
        {
            return Ok(());
        }
        AgentBinder::bind_with_model_override(
            Arc::new(self.agent.definition().clone()),
            AgentInvocation {
                role: AgentRole::Delegated,
                available_tools: self.agent.rho_capabilities().cloned().unwrap_or_default(),
            },
            config,
            Some(ModelOverride::Saved {
                provider: &config.provider,
                model: &config.model,
                auth: &config.auth,
            }),
        )?;
        Ok(())
    }

    /// A host model change replaces a call override even without another turn.
    /// Ordinary ACP roots keep their existing reload policy and deferred files.
    pub(super) fn persist_model_override(&self) -> anyhow::Result<()> {
        let Some(stored) = self.storage.file() else {
            return Ok(());
        };
        let snapshot = self.durable_snapshot();
        if snapshot.metadata().contains_key(MODEL_OVERRIDE_METADATA) {
            stored.save_snapshot(&snapshot, &[])?;
        }
        Ok(())
    }

    /// Rebuilds a fresh child with its selected model and definition's prompt/tools,
    /// then submits only the task to the model. The original command remains
    /// in display history so the host can show how the child was launched.
    pub(super) async fn run_as_agent(
        &mut self,
        display: &UserInput,
        task: AgentTask,
        startup: &AcpStartup,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let catalog = AgentCatalog::discover(self.storage.cwd())?;
        let definition = catalog.find(&task.agent_id)?.definition.clone();
        let model_override = task.model_override.clone();
        let cwd = self.storage.cwd().to_path_buf();
        let added_dirs = self.added_dirs.clone();
        self.run_as_agent_with(
            display,
            task,
            startup,
            client,
            |base| {
                bind_delegated_agent(
                    definition,
                    model_override.as_deref().map(ModelOverride::Reference),
                    startup,
                    base,
                )
            },
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
        task: AgentTask,
        startup: &AcpStartup,
        client: &dyn AcpClientPort,
        bind: impl FnOnce(&Config) -> anyhow::Result<BoundAgent>,
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

        // T3 may select a model, mode and thought level before the first prompt.
        let mut base = model_scoped_config(&self.current_model(), &startup.config);
        base.permission_mode = self.permission.mode;
        base.reasoning = self.built.session.reasoning_level();
        let bound = bind(&base)?;
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
        let saved_override = task
            .model_override
            .as_ref()
            .map(|_| {
                serde_json::to_string(&SavedModelOverride {
                    provider: config.provider.clone(),
                    model: config.model.clone(),
                    auth: config.auth.clone(),
                })
            })
            .transpose()?;
        let mut assembly = build(bound.clone(), config.clone(), id).await?;
        if let Some(saved) = saved_override {
            let snapshot = assembly
                .built
                .session
                .snapshot()
                .with_metadata(MODEL_OVERRIDE_METADATA, saved);
            // SDK snapshot metadata survives checkpoints, compaction and mode
            // rebinds. Seed it before the first turn can save a checkpoint.
            match assembly
                .built
                .runtime
                .rebind_session(SessionOptions::from_snapshot(snapshot))
                .await
            {
                Ok(session) => assembly.built.session = session,
                Err(error) => {
                    assembly.built.teardown().await;
                    return Err(error.into());
                }
            }
        }
        let rebound = self.stored().and_then(|stored| {
            stored
                .rebind_fresh_delegated_agent(bound.id().as_str(), &bound.fingerprint().to_string())
        });
        if let Err(error) = rebound {
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
        self.built.tools.bind_recall(self.storage.file());
        self.refresh_advertisements(&startup.config, client).await;
        self.run_turn(display, task.input, Vec::new(), client).await
    }
}

/// The agent and config a loaded session runs with. ACP roots run as
/// `startup.agent`; a session `/agent:` rebound is marked delegated, so it binds
/// as that child again and must match its saved definition. A changed or
/// missing definition fails the load rather than widening the child to the
/// root's tools. `catalog` discovers definitions only for delegated sessions.
pub(super) fn loaded_agent(
    startup: &AcpStartup,
    stored: &StoredSession,
    config: Config,
    catalog: impl FnOnce() -> anyhow::Result<AgentCatalog>,
) -> anyhow::Result<(BoundAgent, Config)> {
    loaded_agent_with(startup, stored, config, catalog, bind_delegated_agent)
}

/// Keep credential discovery injectable while testing durable selection and
/// definition-identity checks at the ACP load boundary.
fn loaded_agent_with(
    startup: &AcpStartup,
    stored: &StoredSession,
    config: Config,
    catalog: impl FnOnce() -> anyhow::Result<AgentCatalog>,
    bind: impl FnOnce(
        AgentDefinition,
        Option<ModelOverride<'_>>,
        &AcpStartup,
        &Config,
    ) -> anyhow::Result<BoundAgent>,
) -> anyhow::Result<(BoundAgent, Config)> {
    // The persisted role, not the id, decides: a child may share the launch
    // agent's id and must still reload with its delegated tool boundary.
    let Some(stored_id) = stored.delegated_agent_id()? else {
        return Ok((startup.agent.clone(), config));
    };
    let definition = catalog()?.find(&stored_id)?.definition.clone();
    stored.validate_agent_definition_identity(&definition)?;
    let model_override = stored
        .session_metadata(MODEL_OVERRIDE_METADATA)?
        .map(|encoded| serde_json::from_str::<SavedModelOverride>(&encoded))
        .transpose()?;
    let selection = model_override.as_ref().map(|saved| ModelOverride::Saved {
        provider: &saved.provider,
        model: &saved.model,
        auth: &saved.auth,
    });
    let bound = bind(definition, selection, startup, &config)?;
    let config = bound
        .rho_config()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("agent '{stored_id}' is not a Rho agent"))?;
    if let Some(saved) = model_override {
        anyhow::ensure!(
            (config.provider.as_str(), config.model.as_str(), config.auth.as_str())
                == (saved.provider.as_str(), saved.model.as_str(), saved.auth.as_str()),
            "agent '{stored_id}': saved model override {}/{} with auth '{}' resolved to {}/{} with auth '{}'; refusing to change the resumed selection",
            saved.provider, saved.model, saved.auth, config.provider, config.model, config.auth,
        );
    }
    Ok((bound, config))
}

/// Both fresh rebinding and resume must enforce the delegated tool boundary.
fn bind_delegated_agent(
    definition: AgentDefinition,
    model_override: Option<ModelOverride<'_>>,
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
    AgentBinder::bind_with_model_override(
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
        model_override,
    )
}

#[cfg(test)]
#[path = "session_host_agent_tests.rs"]
mod tests;
