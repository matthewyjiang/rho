//! `[internal_agents.<id>]` model overrides for catalog agents whose files the
//! user cannot edit, such as plugin agents.
//!
//! `/agents` writes the entry, bind applies it, and the agents picker shows
//! it. All three ask this module, so they agree on which agents take an entry
//! and how it combines with the file.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::config::{InternalAgentModelConfig, InternalAgentTarget};

use super::{
    is_reserved_agent_id, AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, ModelSelection,
    ReasoningLevel,
};

/// A catalog agent's model as its config entry sets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelOverride {
    /// Replaces the file's model policy, including its `fast` flag.
    pub(crate) model: ModelPolicy,
    /// Replaces the file's reasoning when set.
    pub(crate) reasoning: Option<ReasoningLevel>,
}

/// An entry that cannot apply, because it would switch the agent's runtime.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub(crate) enum ModelOverrideError {
    #[error(
        "agent '{id}' runs on {runtime}, so [internal_agents.{id}] cannot set its model; remove the entry"
    )]
    ExternalRuntime { id: AgentId, runtime: String },
    #[error(
        "agent '{id}': [internal_agents.{id}] selects Claude Code, but only internal agents can switch runtime; name a Rho provider and model"
    )]
    ClaudeCodeTarget { id: AgentId },
}

/// The override `entries` hold for `definition`, if any.
///
/// Ids Rho owns never take one: internal agents and host ids read their
/// entries where they run, and built-ins keep the model they ship with
/// (plugins cannot claim any of these ids). Every other id does, so an entry
/// written for a plugin agent also applies to a user file that takes over the
/// same id.
pub(crate) fn model_override(
    definition: &AgentDefinition,
    entries: &BTreeMap<String, InternalAgentModelConfig>,
) -> Result<Option<ModelOverride>, ModelOverrideError> {
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
            return Err(ModelOverrideError::ExternalRuntime {
                id: id.clone(),
                runtime: definition.runtime.runtime().to_string(),
            })
        }
    }
    match &entry.target {
        InternalAgentTarget::Rho(rho) => Ok(Some(ModelOverride {
            model: ModelPolicy::Select(ModelSelection {
                provider: Some(rho.provider.clone()),
                model: rho.model.clone(),
                auth: Some(rho.auth.clone()),
                fast: false,
            }),
            reasoning: entry.reasoning,
        })),
        InternalAgentTarget::ClaudeCli { .. } => {
            Err(ModelOverrideError::ClaudeCodeTarget { id: id.clone() })
        }
    }
}

/// The model policy and reasoning a Rho agent runs with: the override's when
/// there is one, keeping the file's reasoning unless the override sets its own.
pub(crate) fn effective_rho_model<'a>(
    model: &'a ModelPolicy,
    reasoning: Option<ReasoningLevel>,
    model_override: Option<&'a ModelOverride>,
) -> (&'a ModelPolicy, Option<ReasoningLevel>) {
    match model_override {
        Some(chosen) => (&chosen.model, chosen.reasoning.or(reasoning)),
        None => (model, reasoning),
    }
}
