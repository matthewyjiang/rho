//! Runtime selection model for internal agents: which harness runs an agent,
//! and the settings only that harness understands.
//!
//! The on-disk shape of these selections lives in `config_format`.

use {crate::model_aliases::ModelAliases, rho_providers::reasoning::ReasoningLevel};

/// Provider selection for an internal agent that runs on Rho's own stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RhoInternalAgentModel {
    pub provider: String,
    pub model: String,
    pub auth: String,
    pub(super) model_alias: Option<String>,
}

/// Which harness runs an internal agent, together with the settings only that
/// harness understands.
///
/// The runtime axis travels as one value, mirroring `AgentRuntimeSpec`, so a
/// stored selection cannot pair one harness with another harness's model
/// vocabulary or auth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InternalAgentTarget {
    /// Rho's own provider stack, with an explicit provider, model, and auth.
    Rho(RhoInternalAgentModel),
    /// The installed `claude` binary under the user's Claude Code sign-in.
    /// Rho holds no credential for it and resolves no model name.
    ClaudeCli {
        /// Pass-through `--model`. `None` omits the flag and lets Claude Code
        /// pick.
        model: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InternalAgentModelConfig {
    pub target: InternalAgentTarget,
    /// Per-agent reasoning override. `None` keeps the agent definition default.
    pub reasoning: Option<ReasoningLevel>,
}

impl InternalAgentModelConfig {
    /// A selection on Rho's own runtime.
    pub fn new(provider: String, model: String, auth: String) -> Self {
        Self {
            target: InternalAgentTarget::Rho(RhoInternalAgentModel {
                provider,
                model,
                auth,
                model_alias: None,
            }),
            reasoning: None,
        }
    }

    /// A selection that delegates to the Claude Code CLI. `model` is passed
    /// through as `--model`; `None` lets Claude Code choose.
    pub fn claude_cli(model: Option<String>) -> Self {
        Self {
            target: InternalAgentTarget::ClaudeCli { model },
            reasoning: None,
        }
    }

    /// The Rho provider selection, or `None` when this agent delegates.
    pub fn rho(&self) -> Option<&RhoInternalAgentModel> {
        match &self.target {
            InternalAgentTarget::Rho(model) => Some(model),
            InternalAgentTarget::ClaudeCli { .. } => None,
        }
    }

    /// How the selection reads on a status line, badge, or picker row.
    ///
    /// Both runtimes render as `<source>/<model>` so one row format covers the
    /// whole list.
    pub fn display_reference(&self) -> String {
        match &self.target {
            InternalAgentTarget::Rho(selection) => {
                rho_providers::provider::model_reference(&selection.provider, &selection.model)
            }
            InternalAgentTarget::ClaudeCli { model } => rho_providers::provider::model_reference(
                crate::claude_runtime::models::CLAUDE_CODE_SOURCE_LABEL,
                model
                    .as_deref()
                    .unwrap_or(crate::claude_runtime::models::CLAUDE_DEFAULT_MODEL_BADGE),
            ),
        }
    }

    /// The Rho selection, for assertions that only make sense on Rho's stack.
    #[cfg(test)]
    pub fn expect_rho(&self) -> &RhoInternalAgentModel {
        self.rho().expect("selection runs on the rho runtime")
    }

    /// The Rho selection for mutation in tests.
    #[cfg(test)]
    pub fn expect_rho_mut(&mut self) -> &mut RhoInternalAgentModel {
        match &mut self.target {
            InternalAgentTarget::Rho(model) => model,
            InternalAgentTarget::ClaudeCli { .. } => {
                panic!("selection runs on the rho runtime")
            }
        }
    }

    pub(super) fn current_alias<'a>(&'a self, aliases: &'a ModelAliases) -> Option<&'a str> {
        let selection = self.rho()?;
        let name = selection.model_alias.as_deref()?;
        let target = aliases.get(name)?;
        (target.model == selection.model
            && target.provider.as_deref().unwrap_or(&selection.provider) == selection.provider)
            .then_some(name)
    }
}
