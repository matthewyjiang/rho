use std::path::{Path, PathBuf};

use rho_sdk::SystemPrompt;

use {
    crate::agent::{PromptPolicy, ToolCapability},
    crate::config::Config,
    crate::diagnostics::RuntimeDiagnostics,
    crate::prompt,
    crate::tools::{
        advisor::{advisor_model, AdvisorSessionStore},
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

/// The system prompt with and without the advisor steering text.
///
/// Advisor mode turns on and off mid-session, and the prompt must never
/// describe a tool the run does not have, so both forms are built once and one
/// is selected on every runtime build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SystemPromptVariants {
    without_advisor: SystemPrompt,
    with_advisor: SystemPrompt,
}

impl SystemPromptVariants {
    /// One prompt for both modes, for runs whose prompt cannot carry advisor
    /// steering.
    pub(crate) fn uniform(prompt: SystemPrompt) -> Self {
        Self {
            without_advisor: prompt.clone(),
            with_advisor: prompt,
        }
    }

    pub(crate) fn for_advisor_mode(&self, enabled: bool) -> SystemPrompt {
        if enabled {
            self.with_advisor.clone()
        } else {
            self.without_advisor.clone()
        }
    }
}

/// Capability resolution plus system prompt assembly for root interactive and
/// automation startup. Claude-cli agents bind no Rho host tools; root runs still
/// use the Rho loop and parent config, with Claude execution via AgentExecutor
/// for delegated runs.
pub(crate) fn assemble_tools_and_prompt(
    options: ToolsAndPromptOptions<'_>,
) -> anyhow::Result<(AppToolSet, SystemPromptVariants)> {
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
    let tools = if options.no_tools {
        AppToolSet::disabled()
    } else {
        let mut tool_options = ToolSetOptions::new(capabilities);
        let workflow_tracker = crate::tools::workflow_tracker::WorkflowRunTracker::new();
        tool_options = tool_options.workflow_tracker(workflow_tracker.clone());
        if advisor_capable {
            let store = AdvisorSessionStore::new();
            store.set_model(advisor_model(options.config).cloned());
            tool_options = tool_options.advisor(store);
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
                Some(options.config_path),
                workflow_tracker,
            ));
        }
        AppToolSet::new(options.config, options.diagnostics.clone(), tool_options)
    };
    let specs = tools.specs();
    let system_prompt = if options.no_system_prompt {
        options.diagnostics.update_prompt_sources(Vec::new());
        SystemPromptVariants::uniform(SystemPrompt::None)
    } else {
        let (mut text, mut advisor_text) = match options.agent.prompt() {
            PromptPolicy::Replace(text) => (text.clone(), text.clone()),
            PromptPolicy::Extend(extra) => {
                let mut built = prompt::system_prompt(&specs, options.cwd);
                options.diagnostics.update_prompt_sources(built.sources);
                if !launch_delegation_enabled {
                    prompt::append_subagents_disabled_instruction(&mut built.text);
                }
                let mut advisor_text = built.text.clone();
                prompt::append_advisor_instruction(&mut advisor_text);
                if !extra.is_empty() {
                    let instructions = format!("\n\n# Agent instructions\n\n{extra}");
                    built.text.push_str(&instructions);
                    advisor_text.push_str(&instructions);
                }
                (built.text, advisor_text)
            }
        };
        if text.is_empty() {
            text = "You are a coding agent.".into();
            advisor_text = text.clone();
        }
        SystemPromptVariants {
            without_advisor: SystemPrompt::Custom(text),
            with_advisor: SystemPrompt::Custom(advisor_text),
        }
    };
    if let Some(store) = tools.advisor() {
        // The advisor reviews what the executor was told, and it only ever runs
        // while advisor mode is on, so it reads the advisor variant.
        store.bind_system_prompt(match system_prompt.for_advisor_mode(true) {
            SystemPrompt::Custom(text) => Some(text),
            // `SystemPrompt` is non-exhaustive; only custom text is reviewable.
            SystemPrompt::None | _ => None,
        });
    }
    options.diagnostics.update_tools(&specs);
    Ok((tools, system_prompt))
}

#[cfg(test)]
#[path = "tools_prompt_tests.rs"]
mod tests;
