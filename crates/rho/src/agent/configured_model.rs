//! The configured model of a catalog agent: its `[internal_agents.<id>]`
//! entry, which overrides the file's model. Distinct from a per-call model
//! selection on an `agent` call (`app::agent_binding::ModelOverride`), which
//! wins over both for that call.
//!
//! An entry applies to any Rho catalog agent whose id Rho does not reserve,
//! whatever tier its file comes from (user files included). It exists for
//! agents whose files the user cannot edit, so `/agents` offers the set/clear
//! picker only for plugin agents; for the others the detail pane names the
//! entry. Bind, relaunch, and that detail pane all ask this module, so they
//! agree on which agents take an entry and how it combines with the file.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::config::{InternalAgentModelConfig, InternalAgentTarget};

use super::{
    is_reserved_agent_id, AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, ModelSelection,
    ReasoningLevel,
};

/// A catalog agent's model as its config entry sets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfiguredModel {
    /// Replaces the file's model policy, including its `fast` flag.
    pub(crate) model: ModelPolicy,
    /// Replaces the file's reasoning when set.
    pub(crate) reasoning: Option<ReasoningLevel>,
}

/// An entry that cannot apply, because it would switch the agent's runtime.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub(crate) enum ConfiguredModelError {
    #[error(
        "agent '{id}' runs on {runtime}, so [internal_agents.{id}] cannot set its model; remove the entry"
    )]
    ExternalRuntime { id: AgentId, runtime: String },
    #[error(
        "agent '{id}': [internal_agents.{id}] selects Claude Code, but only internal agents can switch runtime; name a Rho provider and model"
    )]
    ClaudeCodeTarget { id: AgentId },
}

/// The configured model `entries` hold for `definition`, if any.
///
/// Ids Rho owns never take one: internal agents and host ids read their
/// entries where they run, and built-ins keep the model they ship with
/// (plugins cannot claim any of these ids). Every other id does, so an entry
/// written for a plugin agent also applies to a user file that takes over the
/// same id.
pub(crate) fn configured_model(
    definition: &AgentDefinition,
    entries: &BTreeMap<String, InternalAgentModelConfig>,
) -> Result<Option<ConfiguredModel>, ConfiguredModelError> {
    let id = &definition.id;
    if is_reserved_agent_id(id) {
        return Ok(None);
    }
    let Some(entry) = entries.get(id.as_str()) else {
        return Ok(None);
    };
    match &definition.runtime {
        AgentRuntimeSpec::Rho { .. } => {}
        AgentRuntimeSpec::ClaudeCli(_)
        | AgentRuntimeSpec::Cursor(_)
        | AgentRuntimeSpec::Antigravity(_) => {
            return Err(ConfiguredModelError::ExternalRuntime {
                id: id.clone(),
                runtime: definition.runtime.runtime().to_string(),
            })
        }
    }
    match &entry.target {
        InternalAgentTarget::Rho(rho) => Ok(Some(ConfiguredModel {
            model: ModelPolicy::Select(ModelSelection {
                provider: Some(rho.provider.clone()),
                model: rho.model.clone(),
                auth: Some(rho.auth.clone()),
                fast: false,
            }),
            reasoning: entry.reasoning,
        })),
        InternalAgentTarget::ClaudeCli { .. } => {
            Err(ConfiguredModelError::ClaudeCodeTarget { id: id.clone() })
        }
    }
}

/// The model policy and reasoning a Rho agent runs with: the entry's when
/// there is one, keeping the file's reasoning unless the entry sets its own.
pub(crate) fn effective_rho_model<'a>(
    model: &'a ModelPolicy,
    reasoning: Option<ReasoningLevel>,
    configured: Option<&'a ConfiguredModel>,
) -> (&'a ModelPolicy, Option<ReasoningLevel>) {
    match configured {
        Some(chosen) => (&chosen.model, chosen.reasoning.or(reasoning)),
        None => (model, reasoning),
    }
}
