use std::sync::Arc;

use crate::{
    agent::{
        AgentCapabilities, AgentDefinition, AgentFingerprint, AgentId, AgentRuntimeSpec,
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

/// Capacity class for nested concurrency pools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CapacityClass {
    Rho,
    Claude,
}

/// Runtime-specific values produced by binding.
///
/// Callers must match exhaustively so Rho-shaped config and Claude spawn data
/// stay separate after bind.
#[derive(Clone, Debug)]
pub(crate) enum BoundRuntime {
    Rho {
        config: Box<Config>,
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
        /// Claude `--effort` from definition `reasoning:` when set.
        /// `None` means omit the flag (Claude inherit).
        effort: Option<&'static str>,
    },
}

impl BoundRuntime {
    pub(crate) fn capacity_class(&self) -> CapacityClass {
        match self {
            Self::Rho { .. } => CapacityClass::Rho,
            Self::ClaudeCli { .. } => CapacityClass::Claude,
        }
    }

    /// Provider/model labels for the initial status snapshot.
    pub(crate) fn artifact_labels(&self) -> ArtifactLabels {
        match self {
            Self::ClaudeCli { model, .. } => ArtifactLabels {
                provider: "claude-code".into(),
                model: model.clone().unwrap_or_else(|| "claude-cli".into()),
            },
            Self::Rho { config, .. } => ArtifactLabels {
                provider: config.provider.clone(),
                model: config.model.clone(),
            },
        }
    }
}

/// Named provider/model labels from a bound runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactLabels {
    pub(crate) provider: String,
    pub(crate) model: String,
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
            BoundRuntime::Rho { config, .. } => Some(config.as_ref()),
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

    /// Build the Claude session request for a bound Claude runtime.
    pub(crate) fn into_claude_session(
        self,
        prompt: String,
        output_file: std::path::PathBuf,
        cwd: std::path::PathBuf,
        cancellation: rho_tools::cancellation::RunCancellation,
        status_tx: Option<tokio::sync::watch::Sender<crate::subagent::RunStatus>>,
        started_status: Option<crate::subagent::RunStatus>,
    ) -> Option<crate::claude_runtime::session::ClaudeSessionRequest> {
        let BoundRuntime::ClaudeCli {
            model,
            tools,
            inherit_claude_config,
            permission_mode,
            max_turns,
            effort,
        } = self.runtime
        else {
            return None;
        };
        Some(crate::claude_runtime::session::ClaudeSessionRequest {
            system_prompt: self.definition.prompt.clone(),
            identity: crate::claude_runtime::session::ClaudeRunIdentity {
                agent_id: self.definition.id.to_string(),
                agent_fingerprint: self.fingerprint.to_string(),
                model: model.clone(),
            },
            model,
            tools,
            inherit_claude_config,
            permission_mode,
            max_turns,
            effort,
            prompt,
            output_file,
            cwd,
            cancellation,
            status_tx,
            started_status,
            overrides: Default::default(),
        })
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
        let runtime = match &definition.runtime {
            AgentRuntimeSpec::Rho {
                tools,
                model,
                reasoning,
            } => BoundRuntime::Rho {
                config: Box::new(bind_rho_config(
                    definition.id.as_str(),
                    model,
                    *reasoning,
                    host_config,
                )?),
                capabilities: bind_rho_capabilities(&definition, tools, &invocation)?,
            },
            AgentRuntimeSpec::ClaudeCli(config) => {
                bind_claude_runtime(&definition, config, &invocation, host_config)?
            }
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
    tools: &ToolPolicy,
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

    match tools {
        ToolPolicy::All => {
            capabilities.remove(&ToolCapability::Shell);
            Ok(capabilities)
        }
        ToolPolicy::Allow(requested) => {
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
    }
}

fn bind_rho_config(
    agent_id: &str,
    model: &ModelPolicy,
    reasoning: Option<rho_providers::reasoning::ReasoningLevel>,
    host_config: &Config,
) -> anyhow::Result<Config> {
    let mut config = host_config.clone();
    match model {
        ModelPolicy::Inherit => {}
        ModelPolicy::Prefer(selection)
        | ModelPolicy::Require(selection)
        | ModelPolicy::Select(selection) => {
            // Resolve before provider or model-specific handling so all
            // downstream code sees the concrete target.
            let resolved = config
                .model_aliases
                .resolve(&selection.model)
                .map_err(|error| anyhow::anyhow!("agent '{agent_id}': {error}"))?;
            match (&selection.provider, &resolved.provider, &resolved.alias) {
                (Some(pinned), Some(alias_provider), Some(_)) if pinned != alias_provider => {
                    anyhow::bail!(
                        "agent '{agent_id}': model alias '{}' resolves to provider '{alias_provider}', which conflicts with the agent's provider '{pinned}'",
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
    if let Some(reasoning) = reasoning {
        config.reasoning = reasoning;
    }
    Ok(config)
}

fn bind_claude_runtime(
    definition: &AgentDefinition,
    config: &crate::agent::ClaudeAgentConfig,
    invocation: &AgentInvocation,
    host_config: &Config,
) -> anyhow::Result<BoundRuntime> {
    match invocation.role {
        AgentRole::Delegated => {}
        AgentRole::InteractiveRoot | AgentRole::AutomationRoot => {
            anyhow::bail!(
                "agent '{}': runtime claude-cli is delegated-only; use it through the agent tool, not as an interactive or automation root",
                definition.id
            );
        }
    }

    // Defense in depth: parse already rejects these, but constructed configs
    // (tests, future loaders) must not slip past bind.
    if let Some(model) = &config.model {
        if model.starts_with('@') {
            anyhow::bail!(
                "agent '{}': runtime claude-cli does not resolve Rho model aliases; \
set a Claude model name or alias (for example opus), not '{model}'",
                definition.id
            );
        }
    }
    let effort = match config.reasoning {
        None => None,
        Some(level) => match config.effort() {
            Some(effort) => Some(effort),
            None => anyhow::bail!(
                "agent '{}': reasoning '{level}' is not a Claude Code effort level; \
expected one of: low, medium, high, xhigh, max (omit to inherit Claude's default)",
                definition.id
            ),
        },
    };

    // Binder snapshots host permission mode and the shared step budget.
    Ok(BoundRuntime::ClaudeCli {
        model: config.model.clone(),
        tools: config.tools.clone().into_vec(),
        inherit_claude_config: config.inherit_claude_config,
        permission_mode: host_config.permission_mode,
        max_turns: super::sdk_config::run_step_limit()
            .get()
            .try_into()
            .expect("run step limit fits in u64"),
        effort,
    })
}

#[cfg(test)]
#[path = "agent_binding_tests.rs"]
mod tests;
