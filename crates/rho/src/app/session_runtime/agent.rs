//! Agent binding and restoration for host-created children: `/agent:<name>`
//! rebinds a fresh session to a delegated agent, and a reload binds it as
//! that child again.

use std::{future::Future, sync::Arc};

use rho_sdk::{SessionOptions, UserInput};
use serde::{Deserialize, Serialize};

use super::{
    build::{build_session, session_config},
    model_scoped_config, CommandRejected, PromptScope, SessionPermission, SessionRuntime,
};
use crate::{
    agent::{AgentCatalog, AgentDefinition},
    app::{
        agent_binding::{AgentBinder, AgentInvocation, AgentRole, BoundAgent, ModelOverride},
        interactive_runtime::startup::session_options_for_id,
        session_assembly::BuiltSession,
    },
    config::Config,
    session::Session as StoredSession,
};

/// Resolved selection, not the alias or definition: alias edits must not move
/// a resumed call to another provider. Auth is a profile id, never a credential.
/// The key predates the runtime and names ACP, which first hosted children.
pub(in crate::app) const MODEL_OVERRIDE_METADATA: &str = "rho.acp.agent_model_override";

/// `/agent:<name>`: a task for a delegated agent, whose per-call model
/// selection stays separate from the definition.
#[derive(Debug, PartialEq)]
pub(crate) struct AgentTask {
    pub(crate) agent_id: String,
    pub(crate) model_override: Option<String>,
    pub(crate) input: UserInput,
}

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

impl SessionRuntime {
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
    /// Ordinary roots keep their existing reload policy and deferred files.
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

    /// Rebuilds a fresh child for `task` within `prompt`, with its selected
    /// model and definition's prompt and tools. The host then submits only
    /// the task to the model as the prompt's turn; the original command
    /// remains in display history so the host can show how the child was
    /// launched.
    pub(crate) async fn rebind_agent(
        &mut self,
        prompt: &PromptScope,
        task: &AgentTask,
        process_config: &Config,
    ) -> anyhow::Result<()> {
        let catalog = AgentCatalog::discover(self.storage.cwd())?;
        let definition = catalog.find(&task.agent_id)?.definition.clone();
        let model_override = task.model_override.clone();
        let cwd = self.storage.cwd().to_path_buf();
        let added_dirs = self.added_dirs.clone();
        let build = self.build.clone();
        self.rebind_agent_with(
            prompt,
            task,
            process_config,
            |base| {
                bind_delegated_agent(
                    definition,
                    model_override.as_deref().map(ModelOverride::Reference),
                    &build.launch.agent,
                    base,
                )
            },
            async |agent, config, id| {
                Box::pin(build_session(
                    &build,
                    &agent,
                    &config,
                    &cwd,
                    &added_dirs,
                    |_| Ok(session_options_for_id(id)),
                ))
                .await
            },
        )
        .await
    }

    /// Keep the rebind transaction independent of credential/provider discovery
    /// so its session and tool restrictions can use a scripted build in tests.
    /// `build` builds as [`build_session`] does. `_prompt` only proves the
    /// rebind runs within a prompt, whose turn the child then takes.
    pub(crate) async fn rebind_agent_with<Build, BuildFuture>(
        &mut self,
        _prompt: &PromptScope,
        task: &AgentTask,
        process_config: &Config,
        bind: impl FnOnce(&Config) -> anyhow::Result<BoundAgent>,
        build: Build,
    ) -> anyhow::Result<()>
    where
        Build: FnOnce(BoundAgent, Config, rho_sdk::SessionId) -> BuildFuture,
        BuildFuture: Future<Output = anyhow::Result<(BuiltSession, SessionPermission)>>,
    {
        // A conversation must survive, never be dropped.
        if self.completed_runs != 0 || self.has_conversation() {
            return Err(CommandRejected(
                "/agent:<name> starts a fresh session only; create a new session".into(),
            )
            .into());
        }

        // A host may select a model, mode and thought level before the first
        // prompt, as T3 Code does.
        let mut base = model_scoped_config(&self.current_model(), process_config);
        base.permission_mode = self.permission.mode;
        base.reasoning = self.built.session.reasoning_level();
        let bound = bind(&base)?;
        let config = session_config(
            bound.rho_config().ok_or_else(|| not_a_rho_agent(&bound))?,
            &self.host_mcp,
        );
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
        let (mut built, permission) = build(bound.clone(), config.clone(), id).await?;
        if let Some(saved) = saved_override {
            let snapshot = built
                .session
                .snapshot()
                .with_metadata(MODEL_OVERRIDE_METADATA, saved);
            // SDK snapshot metadata survives checkpoints, compaction and mode
            // rebinds. Seed it before the first turn can save a checkpoint.
            match built
                .runtime
                .rebind_session(SessionOptions::from_snapshot(snapshot))
                .await
            {
                Ok(session) => built.session = session,
                Err(error) => {
                    built.teardown().await;
                    return Err(error.into());
                }
            }
        }
        // Publish the concrete selection and delegated identity in one durable
        // replacement, before teardown or host notifications can suspend us.
        let initial_snapshot = task.model_override.as_ref().map(|_| {
            super::persist::DurableState {
                prompt: built.prompt.clone(),
                todo: built.tools.todo_state(),
                code_mode: built.tools.code_mode().clone(),
                added_dirs: self.added_dirs.clone(),
                left_out: self.build.launch.left_out,
                children: self.build.host.children,
                auth: config.auth.clone(),
                // As before the rebind; the next save records the rebound
                // session's.
                settings: self.settings_record(),
            }
            .decorate(built.session.snapshot())
        });
        // An ephemeral session has no file to record the rebind on.
        let rebound = self.stored().and_then(|stored| {
            stored.map_or(Ok(()), |stored| {
                stored.rebind_fresh_delegated_agent(
                    bound.id().as_str(),
                    &bound.fingerprint().to_string(),
                    initial_snapshot.as_ref(),
                )
            })
        });
        if let Err(error) = rebound {
            built.teardown().await;
            return Err(error);
        }
        let old = std::mem::replace(&mut self.built, built);
        old.teardown().await;
        self.permission = permission;
        self.auth = config.auth.clone();
        tracing::debug!(previous = %self.agent.id(), agent = %bound.id(), "rebound session agent");
        self.agent = bound;
        self.built.tools.bind_recall(self.storage.file());
        Ok(())
    }
}

/// The agent and config a loaded session runs with. Roots run as
/// `launch_agent`; a session `/agent:` rebound is marked delegated, so it
/// binds as that child again, capped at `launch_agent`'s tools, and must
/// match its saved definition. A changed or missing definition fails the
/// load rather than widening the child to the root's tools. The host picks
/// `launch_agent`: the daemon binds the root the session was rebound from
/// (see its `bind_saved_agent`), ACP its process's launch agent. `catalog`
/// discovers definitions only for delegated sessions.
pub(super) fn loaded_agent(
    launch_agent: &BoundAgent,
    stored: &StoredSession,
    config: Config,
    catalog: impl FnOnce() -> anyhow::Result<AgentCatalog>,
) -> anyhow::Result<(BoundAgent, Config)> {
    loaded_agent_with(launch_agent, stored, config, catalog, bind_delegated_agent)
}

/// Keep credential discovery injectable while testing durable selection and
/// definition-identity checks at the load boundary.
pub(in crate::app) fn loaded_agent_with(
    launch_agent: &BoundAgent,
    stored: &StoredSession,
    config: Config,
    catalog: impl FnOnce() -> anyhow::Result<AgentCatalog>,
    bind: impl FnOnce(
        AgentDefinition,
        Option<ModelOverride<'_>>,
        &BoundAgent,
        &Config,
    ) -> anyhow::Result<BoundAgent>,
) -> anyhow::Result<(BoundAgent, Config)> {
    // The persisted role, not the id, decides: a child may share the launch
    // agent's id and must still reload with its delegated tool boundary.
    let Some(stored_id) = stored.delegated_agent_id()? else {
        return Ok((launch_agent.clone(), config));
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
    let bound = bind(definition, selection, launch_agent, &config)?;
    let config = bound
        .rho_config()
        .cloned()
        .ok_or_else(|| not_a_rho_agent(&bound))?;
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

/// Both fresh rebinding and resume must enforce the delegated tool boundary:
/// the child gets at most `launch_agent`'s tools. Callers refuse a bound
/// agent that does not run on Rho.
pub(in crate::app) fn bind_delegated_agent(
    definition: AgentDefinition,
    model_override: Option<ModelOverride<'_>>,
    launch_agent: &BoundAgent,
    base: &Config,
) -> anyhow::Result<BoundAgent> {
    AgentBinder::bind_with_model_override(
        Arc::new(definition),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: launch_agent.rho_capabilities().cloned().unwrap_or_default(),
        },
        base,
        model_override,
    )
}

/// Why a session cannot run `bound`: it runs on another runtime, such as
/// Claude Code.
fn not_a_rho_agent(bound: &BoundAgent) -> CommandRejected {
    CommandRejected(format!(
        "{} runs on {}, which a Rho session cannot run",
        bound.id(),
        bound.definition().runtime.runtime()
    ))
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
