use std::{collections::BTreeSet, sync::Arc};

use crate::{
    agent::{AgentDefinition, AgentFingerprint, AgentId, ModelPolicy, PromptPolicy, ToolPolicy},
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
    pub(crate) available_tools: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct BoundAgent {
    definition: Arc<AgentDefinition>,
    fingerprint: AgentFingerprint,
    config: Config,
    tools: BTreeSet<String>,
}

impl BoundAgent {
    pub(crate) fn id(&self) -> &AgentId {
        &self.definition.id
    }

    pub(crate) fn fingerprint(&self) -> AgentFingerprint {
        self.fingerprint
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) fn tools(&self) -> &BTreeSet<String> {
        &self.tools
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
        let mut capabilities = invocation.available_tools;
        if invocation.role == AgentRole::Delegated {
            capabilities.remove("agent");
            capabilities.remove("agents");
            capabilities.remove("questionnaire");
        }

        let tools = match &definition.tools {
            ToolPolicy::All => {
                capabilities.remove("shell");
                capabilities
            }
            ToolPolicy::Allow(requested) => {
                let mut resolved = BTreeSet::new();
                let mut unavailable = Vec::new();
                for tool in requested {
                    if tool == "shell" {
                        let shell = if capabilities.contains("bash") {
                            Some("bash")
                        } else if capabilities.contains("powershell") {
                            Some("powershell")
                        } else {
                            None
                        };
                        if let Some(shell) = shell {
                            resolved.insert(shell.to_string());
                        } else {
                            unavailable.push(tool.clone());
                        }
                    } else if capabilities.contains(tool) {
                        resolved.insert(tool.clone());
                    } else {
                        unavailable.push(tool.clone());
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
                resolved
            }
        };

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

        let fingerprint = definition.fingerprint();
        Ok(BoundAgent {
            definition,
            fingerprint,
            config,
            tools,
        })
    }
}

#[cfg(test)]
#[path = "agent_binding_tests.rs"]
mod tests;
