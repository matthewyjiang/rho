//! Which model runs a piece of work, in the words a prompt or status line states it.
//!
//! Rho knows this in several shapes already: the conversation config, an agent
//! definition's model policy, an internal agent's selection, a finished run's
//! status. Every surface that names a model for a reader routes through this one
//! type, so the executor, its subagents, and the advisor all read the same form.
//!
//! The model id always leads. It is the part a reader can act on: it picks the
//! provider route, it is what `/model` takes back, and it matches what provider
//! documentation calls the model. The catalog name follows in brackets when a
//! catalog carries one, because a model can be newer than whatever is reading
//! the text, and a guessed name is worse than none.
//!
//! Named [`PromptModel`] rather than `ModelIdentity` so it is not confused with
//! the SDK's replay identity (`provider` / `api` / `model`).

use rho_providers::model::display_name::{model_display_name, model_reference_with_display_name};
use rho_sdk::model::ModelIdentity;

use crate::{
    agent::AgentRuntime,
    claude_runtime::models::CLAUDE_CODE_SOURCE_LABEL,
    config::{Config, InternalAgentModelConfig, InternalAgentTarget},
    subagent::RunStatus,
};

/// The model behind one piece of work, named for prompt and status text.
///
/// Values are complete: [`Self::describe`] reads only fields on `self` and the
/// process catalog-name cache. It does not consult ambient "last run" state.
///
/// The runtime axis travels with the model, mirroring `InternalAgentTarget` and
/// `AgentRuntimeSpec`: external CLIs resolve their own model names, so those
/// labels cannot be described in Rho's provider vocabulary alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PromptModel {
    /// A model Rho drives through one of its providers.
    Rho { provider: String, model: String },
    /// An external CLI runtime (Claude Code or Cursor Agent).
    ///
    /// `runtime` is [`AgentRuntime::ClaudeCli`] or [`AgentRuntime::Cursor`].
    /// `requested` is the `--model` value Rho passes through, or `None` when Rho
    /// omits the flag and the CLI chooses. `resolved` is the concrete id a run
    /// reported, when one has. Config and bind paths leave `resolved` empty;
    /// run status fills it from the init frame (`claude_model` for both CLIs).
    ExternalCli {
        runtime: AgentRuntime,
        requested: Option<String>,
        resolved: Option<String>,
    },
}

impl PromptModel {
    /// The model the conversation itself runs on.
    pub(crate) fn from_config(config: &Config) -> Self {
        Self::Rho {
            provider: config.provider.clone(),
            model: config.model.clone(),
        }
    }

    /// The model a live provider reports it is driving.
    pub(crate) fn from_sdk_identity(identity: &ModelIdentity) -> Self {
        Self::Rho {
            provider: identity.provider.clone(),
            model: identity.model.clone(),
        }
    }

    /// The model an internal agent (advisor, session title, goal judge) runs on.
    pub(crate) fn from_internal_agent(selection: &InternalAgentModelConfig) -> Self {
        match &selection.target {
            InternalAgentTarget::Rho(rho) => Self::Rho {
                provider: rho.provider.clone(),
                model: rho.model.clone(),
            },
            InternalAgentTarget::ClaudeCli { model } => Self::ExternalCli {
                runtime: AgentRuntime::ClaudeCli,
                requested: model.clone(),
                resolved: None,
            },
        }
    }

    /// The model a finished or in-flight run recorded on its status.
    ///
    /// Returns `None` when the status has no provider/model pair for a Rho run.
    /// External CLI runs always yield a value: even with nothing pinned and
    /// nothing resolved yet, the label still says the CLI chooses.
    pub(crate) fn from_run_status(status: &RunStatus) -> Option<Self> {
        match status.runtime {
            Some(runtime @ (AgentRuntime::ClaudeCli | AgentRuntime::Cursor)) => {
                Some(Self::ExternalCli {
                    runtime,
                    requested: non_empty(status.model.as_deref()),
                    resolved: non_empty(status.claude_model.as_deref()),
                })
            }
            Some(AgentRuntime::Rho) | None => Some(Self::Rho {
                provider: non_empty(status.provider.as_deref())?,
                model: non_empty(status.model.as_deref())?,
            }),
        }
    }

    /// How the identity reads in prompt or status text.
    ///
    /// Rho models read as `provider/model (Catalog Name)`. External CLI models
    /// read as `<source>/<--model value>`, plus what a run resolved when that
    /// is carried on the value.
    ///
    /// Always one line; see [`one_line`]. Catalog names come from the models.dev
    /// snapshot interactive startup hydrates before the system prompt is built;
    /// mid-session switch notices read the same cache.
    pub(crate) fn describe(&self) -> String {
        one_line(match self {
            Self::Rho { provider, model } => model_reference_with_display_name(provider, model),
            Self::ExternalCli {
                runtime,
                requested,
                resolved,
            } => match runtime {
                AgentRuntime::ClaudeCli => {
                    describe_claude_cli(requested.as_deref(), resolved.as_deref())
                }
                AgentRuntime::Cursor => describe_cursor(requested.as_deref(), resolved.as_deref()),
                AgentRuntime::Rho => {
                    unreachable!("PromptModel::ExternalCli is only for ClaudeCli and Cursor")
                }
            },
        })
    }
}

/// Replaces control characters with spaces.
///
/// Every part of a description comes from outside Rho: provider and model ids
/// from config, catalog names from the models.dev download. Callers write one
/// prompt line or one bracketed notice around this text, and a newline in any
/// part would turn the rest into a line of its own that the executor reads as
/// instructions.
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn one_line(text: String) -> String {
    if !text.contains(char::is_control) {
        return text;
    }
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// Provider whose catalog names Claude Code's models.
///
/// Claude Code runs Anthropic models whatever it bills against, so its resolved
/// ids are looked up under Anthropic even though `claude-code` is what Rho
/// shows as the source.
const CLAUDE_CATALOG_PROVIDER: &str = "anthropic";

fn describe_cursor(requested: Option<&str>, resolved: Option<&str>) -> String {
    use crate::cursor_runtime::models::CURSOR_SOURCE_LABEL;
    match resolved.or(requested) {
        Some(model) => rho_providers::provider::model_reference(CURSOR_SOURCE_LABEL, model),
        None => format!("{CURSOR_SOURCE_LABEL} (no model pinned; Cursor chooses)"),
    }
}

fn describe_claude_cli(requested: Option<&str>, resolved: Option<&str>) -> String {
    match (requested, resolved) {
        // A pinned id that is also the resolved id needs no resolution clause.
        (Some(requested), Some(resolved)) if requested == resolved => {
            claude_reference_with_name(requested)
        }
        (Some(requested), None) => claude_reference_with_name(requested),
        // Requested alias (or other pointer) plus what the run bound.
        (Some(requested), Some(resolved)) => format!(
            "{}, ran as {}",
            rho_providers::provider::model_reference(CLAUDE_CODE_SOURCE_LABEL, requested),
            claude_model_with_name(resolved),
        ),
        (None, Some(resolved)) => format!(
            "{CLAUDE_CODE_SOURCE_LABEL} (no model pinned; ran as {})",
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
