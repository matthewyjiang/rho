use std::sync::Arc;

use crate::{
    agent::{
        AgentCapabilities, AgentDefinition, AgentFingerprint, AgentId, AgentRuntime, AgentTools,
        ModelPolicy, PromptPolicy, ToolCapability, ToolPolicy,
    },
    config::Config,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentRole {
    InteractiveRoot,
    AutomationRoot,
    Delegated,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentInvocation {
    pub(crate) role: AgentRole,
    pub(crate) available_tools: AgentCapabilities,
}

/// Runtime-specific values produced by binding.
///
/// Callers must match exhaustively so Rho-shaped config and Claude spawn data
/// stay separate after bind.
#[derive(Clone, Debug)]
pub(crate) enum BoundRuntime {
    Rho {
        config: Config,
        capabilities: AgentCapabilities,
    },
    ClaudeCli {
        /// Claude `--model` value, byte-for-byte from the definition when set.
        /// `None` means inherit Claude's own default (no `--model` flag).
        model: Option<String>,
        tools: Vec<String>,
        inherit_claude_config: bool,
        /// Snapshot of the parent permission mode at bind time. Claude spawn
        /// maps this; it is not a Rho model/provider config.
        permission_mode: crate::permission::PermissionMode,
        /// Exact Claude `--max-turns` value from the configured step budget.
        max_turns: u64,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct BoundAgent {
    definition: Arc<AgentDefinition>,
    fingerprint: AgentFingerprint,
    runtime: BoundRuntime,
}

impl BoundAgent {
    pub(crate) fn id(&self) -> &AgentId {
        &self.definition.id
    }

    pub(crate) fn fingerprint(&self) -> AgentFingerprint {
        self.fingerprint
    }

    pub(crate) fn definition(&self) -> &AgentDefinition {
        &self.definition
    }

    pub(crate) fn runtime(&self) -> &BoundRuntime {
        &self.runtime
    }

    /// Rho-bound config. Claude-cli agents have no Rho provider/model config.
    pub(crate) fn rho_config(&self) -> Option<&Config> {
        match &self.runtime {
            BoundRuntime::Rho { config, .. } => Some(config),
            BoundRuntime::ClaudeCli { .. } => None,
        }
    }

    /// Rho-bound capabilities. Claude-cli agents do not bind host tools.
    pub(crate) fn rho_capabilities(&self) -> Option<&AgentCapabilities> {
        match &self.runtime {
            BoundRuntime::Rho { capabilities, .. } => Some(capabilities),
            BoundRuntime::ClaudeCli { .. } => None,
        }
    }

    pub(crate) fn prompt(&self) -> &PromptPolicy {
        &self.definition.prompt
    }
}

pub(crate) struct AgentBinder;

impl AgentBinder {
    pub(crate) fn bind(
        definition: Arc<AgentDefinition>,
        invocation: AgentInvocation,
        host_config: &Config,
    ) -> anyhow::Result<BoundAgent> {
        let fingerprint = definition.fingerprint();
        let runtime = match definition.runtime {
            AgentRuntime::Rho => BoundRuntime::Rho {
                config: bind_rho_config(&definition, host_config)?,
                capabilities: bind_rho_capabilities(&definition, &invocation)?,
            },
            AgentRuntime::ClaudeCli => bind_claude_runtime(&definition, &invocation, host_config)?,
        };
        Ok(BoundAgent {
            definition,
            fingerprint,
            runtime,
        })
    }
}

fn bind_rho_capabilities(
    definition: &AgentDefinition,
    invocation: &AgentInvocation,
) -> anyhow::Result<AgentCapabilities> {
    let mut capabilities = invocation.available_tools.clone();
    if invocation.role == AgentRole::Delegated {
        // Keep questionnaire when the host offers it. The executor gates it to
        // background runs with a live parent bridge; foreground and headless
        // paths strip it before bind.
        capabilities.remove(&ToolCapability::Agent);
        capabilities.remove(&ToolCapability::Agents);
    }

    match &definition.tools {
        AgentTools::Rho(ToolPolicy::All) => {
            capabilities.remove(&ToolCapability::Shell);
            Ok(capabilities)
        }
        AgentTools::Rho(ToolPolicy::Allow(requested)) => {
            let mut resolved = crate::agent::ToolCapabilitySet::new();
            let mut unavailable = Vec::new();
            for tool in requested {
                if tool == &ToolCapability::Shell {
                    let shell = if capabilities.contains(&ToolCapability::Bash) {
                        Some(ToolCapability::Bash)
                    } else if capabilities.contains(&ToolCapability::Powershell) {
                        Some(ToolCapability::Powershell)
                    } else {
                        None
                    };
                    if let Some(shell) = shell {
                        resolved.insert(shell);
                    } else {
                        unavailable.push(tool.to_string());
                    }
                } else if capabilities.contains(tool) {
                    resolved.insert(tool.clone());
                } else {
                    unavailable.push(tool.to_string());
                }
            }
            if !unavailable.is_empty() {
                anyhow::bail!(
                    "agent '{}': requested tools are unavailable for {:?}: {}",
                    definition.id,
                    invocation.role,
                    unavailable.join(", ")
                );
            }
            Ok(AgentCapabilities::new(resolved))
        }
        AgentTools::Claude(_) => anyhow::bail!(
            "agent '{}': internal tools/runtime mismatch (rho / claude tools)",
            definition.id
        ),
    }
}

fn bind_rho_config(definition: &AgentDefinition, host_config: &Config) -> anyhow::Result<Config> {
    let mut config = host_config.clone();
    match &definition.model {
        ModelPolicy::Inherit => {}
        ModelPolicy::Prefer(selection)
        | ModelPolicy::Require(selection)
        | ModelPolicy::Select(selection) => {
            // Resolve before provider or model-specific handling so all
            // downstream code sees the concrete target.
            let resolved = config
                .model_aliases
                .resolve(&selection.model)
                .map_err(|error| anyhow::anyhow!("agent '{}': {error}", definition.id))?;
            match (&selection.provider, &resolved.provider, &resolved.alias) {
                (Some(pinned), Some(alias_provider), Some(_)) if pinned != alias_provider => {
                    anyhow::bail!(
                        "agent '{}': model alias '{}' resolves to provider '{alias_provider}', which conflicts with the agent's provider '{pinned}'",
                        definition.id,
                        selection.model,
                    );
                }
                _ => {}
            }
            config.model_alias = resolved.alias;
            let provider = resolved.provider.or_else(|| selection.provider.clone());
            if let Some(provider) = &provider {
                super::cli_config::apply_provider_override(
                    &mut config,
                    provider,
                    /* explicit_model */ true,
                )?;
            }
            config.model = resolved.model;
        }
    }
    if let Some(reasoning) = definition.reasoning {
        config.reasoning = reasoning;
    }
    Ok(config)
}

fn bind_claude_runtime(
    definition: &AgentDefinition,
    invocation: &AgentInvocation,
    host_config: &Config,
) -> anyhow::Result<BoundRuntime> {
    match invocation.role {
        AgentRole::Delegated => {}
        AgentRole::InteractiveRoot | AgentRole::AutomationRoot => {
            anyhow::bail!(
                "agent '{}': runtime claude-cli is delegated-only; \
use it through the agent tool, not as an interactive or automation root",
                definition.id
            );
        }
    }

    if let Some(reasoning) = definition.reasoning {
        anyhow::bail!(
            "agent '{}': runtime claude-cli does not support reasoning: {reasoning}; \
omit reasoning (inherit Claude's default) or use runtime: rho",
            definition.id
        );
    }

    let tools = match &definition.tools {
        AgentTools::Claude(tools) => tools.clone(),
        AgentTools::Rho(_) => anyhow::bail!(
            "agent '{}': internal tools/runtime mismatch (claude-cli / rho tools)",
            definition.id
        ),
    };
    // Claude model is pass-through only. No alias resolution and no parent
    // provider/model mutation. Rho-style `@alias` references are rejected
    // rather than resolved through the host alias table.
    let model = match &definition.model {
        ModelPolicy::Inherit => None,
        ModelPolicy::Select(selection)
        | ModelPolicy::Prefer(selection)
        | ModelPolicy::Require(selection) => {
            if selection.model.starts_with('@') {
                anyhow::bail!(
                    "agent '{}': runtime claude-cli does not resolve Rho model aliases; \
set model to a Claude model name or alias (for example opus), not '{}'",
                    definition.id,
                    selection.model
                );
            }
            Some(selection.model.clone())
        }
    };
    Ok(BoundRuntime::ClaudeCli {
        model,
        tools,
        inherit_claude_config: definition.inherit_claude_config,
        permission_mode: host_config.permission_mode,
        // Same application step budget Rho delegated agents use when no
        // explicit max_steps override is present.
        max_turns: super::sdk_config::run_step_limit()
            .get()
            .try_into()
            .expect("run step limit fits in u64"),
    })
}

#[cfg(test)]
#[path = "agent_binding_tests.rs"]
mod tests;
