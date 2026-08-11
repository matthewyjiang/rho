//! Which model runs a piece of work, in the words a prompt states it.
//!
//! Rho knows this in several shapes already: the conversation config, an agent
//! definition's model policy, an internal agent's selection. Every prompt that
//! names a model routes through this one type, so the executor, its subagents,
//! and the advisor all read the same form and one change fixes them all.
//!
//! The model id always leads. It is the part a reader can act on: it picks the
//! provider route, it is what `/model` takes back, and it matches what provider
//! documentation calls the model. The catalog name follows in brackets when a
//! catalog carries one, because a model can be newer than whatever is reading
//! the prompt, and a guessed name is worse than none.

use rho_providers::model::display_name::{model_display_name, model_reference_with_display_name};

use crate::{
    agent::{AgentDefinition, AgentRuntimeSpec},
    claude_runtime::{models::CLAUDE_CODE_SOURCE_LABEL, resolved_models},
    config::{Config, InternalAgentModelConfig, InternalAgentTarget},
};

/// The model behind one piece of work, named for a prompt.
///
/// The runtime axis travels with the model, mirroring `InternalAgentTarget` and
/// `AgentRuntimeSpec`: Claude Code resolves its own model names, so its
/// identity cannot be described in Rho's provider vocabulary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelIdentity {
    /// A model Rho drives through one of its providers.
    Rho { provider: String, model: String },
    /// The Claude Code CLI. `model` is the `--model` value Rho passes through,
    /// or `None` when Rho omits the flag and Claude Code chooses.
    ClaudeCli { model: Option<String> },
}

impl ModelIdentity {
    /// The model the conversation itself runs on.
    pub(crate) fn from_config(config: &Config) -> Self {
        Self::Rho {
            provider: config.provider.clone(),
            model: config.model.clone(),
        }
    }

    /// The model an internal agent (advisor, session title, goal judge) runs on.
    pub(crate) fn from_internal_agent(selection: &InternalAgentModelConfig) -> Self {
        match &selection.target {
            InternalAgentTarget::Rho(rho) => Self::Rho {
                provider: rho.provider.clone(),
                model: rho.model.clone(),
            },
            InternalAgentTarget::ClaudeCli { model } => Self::ClaudeCli {
                model: model.clone(),
            },
        }
    }

    /// The model an agent definition will run on under `host`.
    ///
    /// This predicts what binding will choose, for descriptions written before
    /// any launch. It answers provider and model only; binding also settles
    /// auth, reasoning, and tools. `agent_binding_tests` holds the two paths to
    /// the same answer.
    pub(crate) fn for_agent(definition: &AgentDefinition, host: &Config) -> Self {
        let policy = match &definition.runtime {
            AgentRuntimeSpec::ClaudeCli(claude) => {
                return Self::ClaudeCli {
                    model: claude.model.clone(),
                }
            }
            AgentRuntimeSpec::Rho { model, .. } => model,
        };
        let Some(selection) = policy.selection() else {
            return Self::from_config(host);
        };
        // An alias that does not resolve is a bind-time error with its own
        // message. A description states what the definition asked for rather
        // than inventing a target.
        let resolved = host.model_aliases.resolve(&selection.model).ok();
        let provider = resolved
            .as_ref()
            .and_then(|resolved| resolved.provider.clone())
            .or_else(|| selection.provider.clone())
            .unwrap_or_else(|| host.provider.clone());
        let model = resolved
            .map(|resolved| resolved.model)
            .unwrap_or_else(|| selection.model.clone());
        Self::Rho { provider, model }
    }

    /// How the identity reads in prompt text.
    ///
    /// Rho models read as `provider/model (Catalog Name)`. Claude Code models
    /// read as `claude-code/<--model value>`, plus what that value resolved to
    /// when a run has reported it.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Rho { provider, model } => model_reference_with_display_name(provider, model),
            Self::ClaudeCli { model } => describe_claude_cli(model.as_deref()),
        }
    }
}

/// Every model this session can name, as catalog lookup keys.
///
/// Names come from a cache that only a model *selection* fills, so a model that
/// is only ever a subagent target would never get one. This lists what a session
/// will describe - the conversation model, every agent in the catalog, and every
/// internal agent - so one prefetch can cover them all.
///
/// Claude Code models are absent. Rho cannot resolve `--model opus` to an id
/// before a run reports one, and an unresolved alias has nothing to look up.
pub(crate) fn describable_models(
    config: &Config,
    catalog: &crate::agent::AgentCatalog,
) -> Vec<(String, String)> {
    let agents = catalog
        .iter()
        .map(|entry| ModelIdentity::for_agent(&entry.definition, config));
    let internal_agents = config
        .internal_agents
        .values()
        .map(ModelIdentity::from_internal_agent);
    std::iter::once(ModelIdentity::from_config(config))
        .chain(agents)
        .chain(internal_agents)
        .filter_map(|identity| match identity {
            ModelIdentity::Rho { provider, model } => Some((provider, model)),
            ModelIdentity::ClaudeCli { .. } => None,
        })
        .collect()
}

/// Provider whose catalog names Claude Code's models.
///
/// Claude Code runs Anthropic models whatever it bills against, so its resolved
/// ids are looked up under Anthropic even though `claude-code` is what Rho
/// shows as the source.
const CLAUDE_CATALOG_PROVIDER: &str = "anthropic";

fn describe_claude_cli(requested: Option<&str>) -> String {
    let resolved = resolved_models::last_resolved(requested);
    match (requested, resolved.as_deref()) {
        // A pinned id that ran as itself needs no resolution clause, only its
        // name. The same holds before any run reports one.
        (Some(requested), Some(resolved)) if requested == resolved => {
            claude_reference_with_name(requested)
        }
        (Some(requested), None) => claude_reference_with_name(requested),
        // An alias points at whichever model is current, so what a run reported
        // is the last answer rather than a standing one.
        (Some(requested), Some(resolved)) => format!(
            "{}, last ran as {}",
            rho_providers::provider::model_reference(CLAUDE_CODE_SOURCE_LABEL, requested),
            claude_model_with_name(resolved),
        ),
        (None, Some(resolved)) => format!(
            "{CLAUDE_CODE_SOURCE_LABEL} (no model pinned; last ran as {})",
            claude_model_with_name(resolved),
        ),
        (None, None) => {
            format!("{CLAUDE_CODE_SOURCE_LABEL} (no model pinned; Claude Code chooses)")
        }
    }
}

/// `claude-code/<model>` plus the catalog name when one is known.
fn claude_reference_with_name(model: &str) -> String {
    let reference = rho_providers::provider::model_reference(CLAUDE_CODE_SOURCE_LABEL, model);
    match model_display_name(CLAUDE_CATALOG_PROVIDER, model) {
        Some(name) => format!("{reference} ({name})"),
        None => reference,
    }
}

/// A bare Claude model id plus its catalog name, for use inside a clause that
/// already named the source.
fn claude_model_with_name(model: &str) -> String {
    match model_display_name(CLAUDE_CATALOG_PROVIDER, model) {
        Some(name) => format!("{model} ({name})"),
        None => model.to_string(),
    }
}

#[cfg(test)]
#[path = "model_identity_tests.rs"]
mod tests;
