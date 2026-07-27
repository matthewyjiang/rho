use std::path::{Path, PathBuf};

use rho_sdk::SystemPrompt;

use {
    crate::agent::{PromptPolicy, ToolCapability},
    crate::config::Config,
    crate::diagnostics::RuntimeDiagnostics,
    crate::prompt,
    crate::tools::{
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
    pub(crate) background_subagents: BackgroundSubagents,
    pub(crate) diagnostics: &'a RuntimeDiagnostics,
    pub(crate) agent: &'a BoundAgent,
}

/// Capability resolution plus system prompt assembly for root interactive and
/// automation startup. Claude-cli agents bind no Rho host tools; root runs still
/// use the Rho loop and parent config, with Claude execution via AgentExecutor
/// for delegated runs.
pub(crate) fn assemble_tools_and_prompt(
    options: ToolsAndPromptOptions<'_>,
) -> anyhow::Result<(AppToolSet, SystemPrompt)> {
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
    let launch_delegation_enabled = capabilities.contains(&ToolCapability::Agent);
    let delegation_enabled =
        launch_delegation_enabled || capabilities.contains(&ToolCapability::Agents);
    let tools = if options.no_tools {
        AppToolSet::disabled()
    } else {
        let mut tool_options = ToolSetOptions::new(capabilities);
        if delegation_enabled {
            tool_options = tool_options.delegation(DelegationConfig::new(
                options.cwd.to_path_buf(),
                options.config_path,
                options.background_subagents,
            ));
        }
        AppToolSet::new(options.config, options.diagnostics.clone(), tool_options)
    };
    let specs = tools.specs();
    let system_prompt = if options.no_system_prompt {
        options.diagnostics.update_prompt_sources(Vec::new());
        SystemPrompt::None
    } else {
        let mut text = match options.agent.prompt() {
            PromptPolicy::Replace(text) => text.clone(),
            PromptPolicy::Extend(extra) => {
                let mut built = prompt::system_prompt(&specs, options.cwd);
                options.diagnostics.update_prompt_sources(built.sources);
                if !launch_delegation_enabled {
                    prompt::append_subagents_disabled_instruction(&mut built.text);
                }
                if !extra.is_empty() {
                    built.text.push_str("\n\n# Agent instructions\n\n");
                    built.text.push_str(extra);
                }
                built.text
            }
        };
        if text.is_empty() {
            text = "You are a coding agent.".into();
        }
        SystemPrompt::Custom(text)
    };
    options.diagnostics.update_tools(&specs);
    Ok((tools, system_prompt))
}
