use std::path::{Path, PathBuf};

use rho_sdk::SystemPrompt;

use {
    crate::agent::{PromptPolicy, ToolCapability},
    crate::config::Config,
    crate::diagnostics::RuntimeDiagnostics,
    crate::prompt,
    crate::tools::{
        advisor::AdvisorSessionStore,
        agent::BackgroundSubagents,
        sdk_registry::{AppToolSet, DelegationConfig, ToolSetOptions},
    },
};

use super::agent_binding::BoundAgent;

pub(crate) struct ToolsAndPromptOptions<'a> {
    pub(crate) config: &'a Config,
    pub(crate) config_path: PathBuf,
    pub(crate) cwd: &'a Path,
    pub(crate) no_system_prompt: bool,
    pub(crate) no_tools: bool,
    pub(crate) no_subagents: bool,
    pub(crate) questionnaire_enabled: bool,
    /// Whether this run can put an MCP server's question in front of a person.
    /// Only a host that answers questionnaires may declare `elicitation`.
    pub(crate) mcp_elicitation: crate::tools::mcp::McpElicitationSupport,
    /// Whether this run will bind a model that opted-in MCP servers may sample.
    pub(crate) mcp_sampling: McpSamplingSupport,
    pub(crate) background_subagents: BackgroundSubagents,
    pub(crate) diagnostics: &'a RuntimeDiagnostics,
    pub(crate) agent: &'a BoundAgent,
}

/// Whether this run offers MCP sampling at all.
///
/// A run that never binds a model must not declare the `sampling` capability,
/// so the decision is made by the host rather than inferred later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpSamplingSupport {
    Available,
    Unavailable,
}
pub(crate) struct StartupInventory {
    pub(crate) mcp: crate::tools::mcp::McpSessionReport,
    pub(crate) plugins: crate::plugins::PluginLoadReport,
}

pub(crate) struct ToolsAndPrompt {
    pub(crate) tools: AppToolSet,
    pub(crate) system_prompt: SystemPromptVariants,
    pub(crate) inventory: StartupInventory,
    /// Late-bound model handle for MCP sampling. Bound once the runtime exists,
    /// and rebound whenever the user changes models. Left unbound, every
    /// sampling request fails closed.
    pub(crate) mcp_sampling: crate::tools::mcp::McpSamplingBridge,
}

/// Fixed executor system prompt for a session.
///
/// Mid-session tool changes keep this prompt fixed for prompt-cache stability
/// and tell the model about tool list changes with appended context instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SystemPromptVariants {
    prompt: SystemPrompt,
}

impl SystemPromptVariants {
    pub(crate) fn uniform(prompt: SystemPrompt) -> Self {
        Self { prompt }
    }

    pub(crate) fn get(&self) -> SystemPrompt {
        self.prompt.clone()
    }
}

/// Capability resolution plus system prompt assembly for root interactive and
/// automation startup. Claude-cli agents bind no Rho host tools; root runs still
/// use the Rho loop and parent config, with Claude execution via AgentExecutor
/// for delegated runs.
pub(crate) async fn assemble_tools_and_prompt(
    options: ToolsAndPromptOptions<'_>,
) -> anyhow::Result<ToolsAndPrompt> {
    let native_runtime = options.agent.rho_capabilities().is_some();
    let mut capabilities = options
        .agent
        .rho_capabilities()
        .cloned()
        .unwrap_or_default();
    if options.no_subagents {
        capabilities.remove(&ToolCapability::Agent);
        capabilities.remove(&ToolCapability::Agents);
    }
    if !options.questionnaire_enabled {
        capabilities.remove(&ToolCapability::Questionnaire);
    }
    // The capability says the run may offer the advisor; config says whether it
    // does right now. Advisor mode on with no advisor model configured is a real
    // state, and the TUI surfaces it.
    let advisor_capable = capabilities.contains(&ToolCapability::Advisor);
    let launch_delegation_enabled = capabilities.contains(&ToolCapability::Agent);
    let delegation_enabled =
        launch_delegation_enabled || capabilities.contains(&ToolCapability::Agents);
    // Agent Plugins contribute skills through ordinary skill discovery and
    // MCP servers through the generic native MCP configuration.
    let plugin_discovery =
        crate::plugins::discover(options.cwd, crate::paths::home_dir().as_deref());
    crate::plugins::log(&plugin_discovery.report);
    let crate::plugins::PluginDiscovery {
        skills: plugin_skills,
        mcp: plugin_mcp,
        report: plugins_report,
    } = plugin_discovery;
    let mut mcp_config = options.config.mcp.clone();
    mcp_config.merge(plugin_mcp);
    let mcp_plan = if !native_runtime {
        crate::tools::mcp::McpSessionPlan::Inventory(
            crate::tools::mcp::McpLoadMode::UnsupportedAgent,
        )
    } else if options.no_tools {
        crate::tools::mcp::McpSessionPlan::Inventory(crate::tools::mcp::McpLoadMode::ToolsDisabled)
    } else {
        crate::tools::mcp::McpSessionPlan::Connect
    };
    let mcp_sampling_bridge = crate::tools::mcp::McpSamplingBridge::new();
    let mut mcp_options = crate::tools::mcp::McpSessionOptions::new(
        options.config.max_output_bytes,
        crate::tools::mcp::McpRoots::for_workspace(options.cwd),
        crate::tools::mcp::McpAuthorizationMode::from_process(),
    )
    .with_elicitation(options.mcp_elicitation);
    match options.mcp_sampling {
        McpSamplingSupport::Available => {
            mcp_options = mcp_options.with_sampling(mcp_sampling_bridge.clone());
        }
        McpSamplingSupport::Unavailable => {}
    }
    let mcp = crate::tools::mcp::McpConnectOutcome::run(mcp_plan, &mcp_config, mcp_options).await;
    let tools = if options.no_tools {
        AppToolSet::disabled().with_mcp(mcp)
    } else {
        let mut tool_options = ToolSetOptions::new(capabilities);
        let workflow_tracker = crate::tools::workflow_tracker::WorkflowRunTracker::new();
        tool_options = tool_options.workflow_tracker(workflow_tracker.clone());
        if advisor_capable {
            tool_options = tool_options.advisor(AdvisorSessionStore::new());
        }
        if delegation_enabled {
            tool_options = tool_options.delegation(DelegationConfig::new(
                options.cwd.to_path_buf(),
                options.config_path.clone(),
                options.background_subagents,
            ));
        }
        if options
            .agent
            .rho_capabilities()
            .is_some_and(|capabilities| capabilities.contains(&ToolCapability::Workflow))
        {
            tool_options = tool_options.workflow(super::workflow_cli::workflow_tool_service(
                options.cwd.to_path_buf(),
                Some(options.config_path.clone()),
                workflow_tracker,
            ));
        }
        AppToolSet::new(options.config, options.diagnostics.clone(), tool_options).with_mcp(mcp)
    };
    let mcp_report = tools.mcp_report().clone();
    let specs = tools.specs();
    let system_prompt = if options.no_system_prompt {
        options.diagnostics.update_prompt_sources(Vec::new());
        SystemPromptVariants::uniform(SystemPrompt::None)
    } else {
        let mut text = match options.agent.prompt() {
            PromptPolicy::Replace(text) => text.clone(),
            PromptPolicy::Extend(extra) => {
                let mut built =
                    prompt::system_prompt_with_plugin_skills(&specs, options.cwd, plugin_skills);
                options.diagnostics.update_prompt_sources(built.sources);
                if !launch_delegation_enabled {
                    prompt::append_subagents_disabled_instruction(&mut built.text);
                }
                // Server guidance describes the MCP tools this run actually has.
                let mcp_instructions = mcp_report
                    .servers
                    .iter()
                    .filter_map(|server| Some((server.identity.as_str(), server.instructions()?)))
                    .collect::<Vec<_>>();
                prompt::append_mcp_instructions(&mut built.text, mcp_instructions.iter().copied());
                if !extra.is_empty() {
                    built
                        .text
                        .push_str(&format!("\n\n# Agent instructions\n\n{extra}"));
                }
                built.text
            }
        };
        if text.is_empty() {
            text = "You are a coding agent.".into();
        }
        // Advisor steering is frozen into the system prompt only when the tool
        // is already registered at session start. Mid-session /advisor toggles
        // keep this prompt fixed and announce changes with context notices.
        if tools.advisor_registered() {
            prompt::append_advisor_instruction(&mut text);
        }
        SystemPromptVariants::uniform(SystemPrompt::Custom(text))
    };
    if let Some(store) = tools.advisor() {
        // The advisor reviews what the executor was told.
        store.bind_system_prompt(match system_prompt.get() {
            SystemPrompt::Custom(text) => Some(text),
            // `SystemPrompt` is non-exhaustive; only custom text is reviewable.
            _ => None,
        });
    }
    options.diagnostics.update_tools(&specs);
    Ok(ToolsAndPrompt {
        tools,
        system_prompt,
        inventory: StartupInventory {
            mcp: mcp_report,
            plugins: plugins_report,
        },
        mcp_sampling: mcp_sampling_bridge,
    })
}

#[cfg(test)]
#[path = "tools_prompt_tests.rs"]
mod tests;
