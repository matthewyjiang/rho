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
    Workflow,
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
    step_limit: u64,
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

    pub(crate) fn step_limit(&self) -> u64 {
        self.step_limit
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
            } => {
                let config = Box::new(bind_rho_config(
                    definition.id.as_str(),
                    model,
                    *reasoning,
                    host_config,
                )?);
                let available_tools =
                    available_tools_for_bound_config(&invocation.available_tools, config.as_ref());
                BoundRuntime::Rho {
                    capabilities: bind_rho_capabilities(
                        &definition,
                        tools,
                        &AgentInvocation {
                            role: invocation.role,
                            available_tools,
                        },
                    )?,
                    config,
                }
            }
            AgentRuntimeSpec::ClaudeCli(config) => {
                bind_claude_runtime(&definition, config, &invocation, host_config)?
            }
        };
        Ok(BoundAgent {
            definition,
            fingerprint,
            runtime,
            step_limit: super::sdk_config::run_step_limit().get() as u64,
        })
    }

    /// Rebuilds a launch object only from metadata stored in a frozen graph.
    ///
    /// This path does not discover, open, or bind an agent definition. Current
    /// config supplies credentials and can narrow permission mode, but every
    /// provider, model, prompt, capability, and step choice comes from `frozen`.
    pub(crate) fn bind_frozen(
        frozen: &crate::workflow::ResolvedAgent,
        current_config: &Config,
        current_tools: &AgentCapabilities,
    ) -> anyhow::Result<BoundAgent> {
        let id = AgentId::new(frozen.agent_id.clone())?;
        let fingerprint = frozen.fingerprint.parse::<AgentFingerprint>()?;
        let prompt = decode_frozen_prompt_policy(&frozen.prompt_policy)?;
        let permission_mode = narrower_permission_mode(
            parse_permission_mode(&frozen.permission_ceiling)?,
            current_config.permission_mode,
        );
        let definition = Arc::new(AgentDefinition {
            id,
            description: "frozen workflow agent".into(),
            prompt,
            runtime: AgentRuntimeSpec::default(),
        });
        let runtime = match frozen.runtime {
            crate::workflow::AgentRuntime::Rho => {
                let mut config = current_config.clone();
                if let Some(provider) = &frozen.provider {
                    config.provider.clone_from(provider);
                }
                if let Some(model) = &frozen.model {
                    config.model.clone_from(model);
                }
                if let Some(reasoning) = &frozen.reasoning {
                    config.reasoning = reasoning.parse().map_err(|_| {
                        anyhow::anyhow!("frozen agent reasoning is invalid: '{reasoning}'")
                    })?;
                }
                if let Some(auth) = &frozen.auth_profile {
                    config.auth.clone_from(auth);
                }
                config.permission_mode = permission_mode;
                let available_tools = available_tools_for_bound_config(current_tools, &config);
                let capabilities = frozen_capabilities(frozen, &available_tools);
                BoundRuntime::Rho {
                    config: Box::new(config),
                    capabilities,
                }
            }
            crate::workflow::AgentRuntime::ClaudeCli => {
                let effort = frozen
                    .reasoning
                    .as_deref()
                    .map(|reasoning| reasoning.parse())
                    .transpose()
                    .map_err(|_| anyhow::anyhow!("frozen Claude reasoning is invalid"))?
                    .and_then(crate::claude_runtime::spawn::claude_effort_flag);
                BoundRuntime::ClaudeCli {
                    model: frozen.model.clone(),
                    tools: frozen.capabilities.iter().cloned().collect(),
                    inherit_claude_config: false,
                    permission_mode,
                    max_turns: frozen.step_limit,
                    effort,
                }
            }
        };
        Ok(BoundAgent {
            definition,
            fingerprint,
            runtime,
            step_limit: frozen.step_limit,
        })
    }
}

fn decode_frozen_prompt_policy(encoded: &str) -> anyhow::Result<PromptPolicy> {
    if let Some(text) = encoded.strip_prefix("extend:") {
        Ok(PromptPolicy::Extend(text.to_owned()))
    } else if let Some(text) = encoded.strip_prefix("replace:") {
        Ok(PromptPolicy::Replace(text.to_owned()))
    } else {
        anyhow::bail!("frozen prompt policy is invalid")
    }
}

fn frozen_capabilities(
    frozen: &crate::workflow::ResolvedAgent,
    current_tools: &AgentCapabilities,
) -> AgentCapabilities {
    let mut tools = crate::agent::ToolCapabilitySet::new();
    for name in &frozen.capabilities {
        let capability = ToolCapability::parse(name.clone());
        if current_tools.contains(&capability) {
            tools.insert(capability);
        }
    }
    let mut capabilities = AgentCapabilities::new(tools);
    for forbidden in [
        ToolCapability::Advisor,
        ToolCapability::Agent,
        ToolCapability::Agents,
        ToolCapability::Questionnaire,
        ToolCapability::Rho,
        ToolCapability::Workflow,
    ] {
        capabilities.remove(&forbidden);
    }
    capabilities
}

fn parse_permission_mode(value: &str) -> anyhow::Result<crate::permission::PermissionMode> {
    value
        .parse()
        .map_err(|error| anyhow::anyhow!("frozen permission ceiling is invalid: {error}"))
}

pub(crate) fn narrower_permission_mode(
    frozen: crate::permission::PermissionMode,
    current: crate::permission::PermissionMode,
) -> crate::permission::PermissionMode {
    use crate::permission::PermissionMode;
    let rank = |mode| match mode {
        PermissionMode::Plan => 0,
        PermissionMode::Supervised => 1,
        PermissionMode::Auto => 2,
    };
    if rank(current) < rank(frozen) {
        current
    } else {
        frozen
    }
}

fn bind_rho_capabilities(
    definition: &AgentDefinition,
    tools: &ToolPolicy,
    invocation: &AgentInvocation,
) -> anyhow::Result<AgentCapabilities> {
    let mut capabilities = invocation.available_tools.clone();
    if matches!(invocation.role, AgentRole::Delegated | AgentRole::Workflow) {
        // Keep questionnaire when the host offers it. The executor gates it to
        // background runs with a live parent bridge; foreground and headless
        // paths strip it before bind.
        capabilities.remove(&ToolCapability::Agent);
        capabilities.remove(&ToolCapability::Agents);
        // The advisor reviews the root session. A child run has its own
        // session and nothing to review.
        capabilities.remove(&ToolCapability::Advisor);
    }
    if invocation.role == AgentRole::Workflow {
        for capability in [
            ToolCapability::Advisor,
            ToolCapability::Agent,
            ToolCapability::Agents,
            ToolCapability::Questionnaire,
            ToolCapability::Rho,
            ToolCapability::Workflow,
        ] {
            capabilities.remove(&capability);
        }
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

/// Drop `web_search` when the bound provider/model cannot use hosted or backup search.
///
/// Host available tools are the ceiling. Callers should leave `web_search` in that
/// set when host tools are enabled; bind removes it if the bound config cannot
/// run search.
fn available_tools_for_bound_config(
    host_tools: &AgentCapabilities,
    bound_config: &Config,
) -> AgentCapabilities {
    let mut tools = host_tools.clone();
    if !crate::tools::web::web_search_available(bound_config) {
        tools.remove(&ToolCapability::WebSearch);
    }
    tools
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
        AgentRole::Delegated | AgentRole::Workflow => {}
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
