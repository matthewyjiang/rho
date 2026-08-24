//! Deferred MCP connect and pre-request system prompt refresh.

use futures_util::FutureExt;
use rho_sdk::{model::Message, SystemPrompt};

use super::InteractiveRuntime;
use crate::{
    agent::{PromptPolicy, ToolCapability},
    model_identity::PromptModel,
    prompt,
    tools::mcp::McpConnectOutcome,
};

impl InteractiveRuntime {
    pub(crate) fn mcp_connect_pending(&self) -> bool {
        self.pending_mcp.is_some()
    }

    pub(crate) fn startup_hydrate_pending(&self) -> bool {
        self.pending_mcp.is_some() || self.pending_catalog_names.is_some()
    }

    pub(crate) fn cancel_startup_hydrates(&mut self) {
        if let Some(handle) = self.pending_mcp.take() {
            handle.abort();
        }
        if let Some(handle) = self.pending_catalog_names.take() {
            handle.abort();
        }
    }

    /// Apply finished MCP connect and catalog-name hydrates. Returns whether
    /// `/mcp` inventory or the startup prompt changed.
    pub(crate) async fn poll_startup_hydrates(&mut self) -> anyhow::Result<bool> {
        let mut changed = false;
        if let Some(handle) = self.pending_mcp.as_mut() {
            if handle.is_finished() {
                if let Some(handle) = self.pending_mcp.take() {
                    match handle.now_or_never() {
                        Some(Ok(outcome)) => {
                            self.apply_mcp_connect(outcome).await?;
                            changed = true;
                        }
                        // The connect task died without reporting. Say so, or
                        // `/mcp` sits on `connecting` until the session ends.
                        Some(Err(error)) => {
                            self.mcp_report
                                .fail_connecting(&format!("MCP connect did not finish: {error}"));
                            changed = true;
                        }
                        None => {}
                    }
                }
            }
        }
        if let Some(handle) = self.pending_catalog_names.as_mut() {
            if handle.is_finished() {
                if let Some(handle) = self.pending_catalog_names.take() {
                    let _ = handle.now_or_never();
                    if self.pending_mcp.is_none() {
                        changed |= self.refresh_startup_system_prompt()?;
                    }
                }
            }
        }
        Ok(changed)
    }

    async fn apply_mcp_connect(&mut self, outcome: McpConnectOutcome) -> anyhow::Result<()> {
        let instructions = outcome
            .report
            .servers
            .iter()
            .filter_map(|server| Some((server.identity.as_str(), server.instructions()?)))
            .map(|(identity, text)| (identity.to_string(), text.to_string()))
            .collect::<Vec<_>>();
        self.tools.attach_mcp(outcome);
        self.mcp_report = self.tools.mcp_report().clone();
        let prompt_changed = self.refresh_startup_system_prompt()?;
        self.rebind_current_session().await?;
        self.remember_tool_list();
        if prompt_changed {
            self.replace_history_system_prompt()?;
        } else if !instructions.is_empty() && !self.may_rewrite_startup_prompt {
            let mut notice = String::new();
            prompt::append_mcp_instructions(
                &mut notice,
                instructions
                    .iter()
                    .map(|(identity, text)| (identity.as_str(), text.as_str())),
            );
            if !notice.is_empty() {
                let _ = self.append_user_context_with_display(notice.clone(), notice);
            }
        }
        Ok(())
    }

    fn refresh_startup_system_prompt(&mut self) -> anyhow::Result<bool> {
        if !self.may_rewrite_startup_prompt || self.live_context_warm {
            return Ok(false);
        }
        let PromptPolicy::Extend(extra) = self.agent.prompt() else {
            return Ok(false);
        };
        let extra = extra.clone();
        let running = self.agent.prompt_model();
        let advisor_capable = self
            .agent
            .rho_capabilities()
            .is_some_and(|capabilities| capabilities.contains(&ToolCapability::Advisor));
        let advisor = advisor_capable
            .then(|| crate::tools::advisor::advisor_model(&self.config))
            .flatten()
            .map(PromptModel::from_internal_agent);
        let plugin_skills =
            crate::plugins::discover(self.workspace.root(), crate::paths::home_dir().as_deref())
                .skills;
        let specs = self.tools.specs();
        let mut built = prompt::system_prompt_with_plugin_skills(
            &specs,
            self.workspace.root(),
            prompt::PromptModels {
                running: &running,
                advisor: advisor.as_ref(),
            },
            plugin_skills,
        );
        if !self.has_tool("agent") {
            prompt::append_subagents_disabled_instruction(&mut built.text);
        }
        let mcp_instructions = self
            .mcp_report
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
        if built.text.is_empty() {
            built.text = "You are a coding agent.".into();
        }
        let next = SystemPrompt::Custom(built.text);
        if next == self.system_prompt {
            return Ok(false);
        }
        self.system_prompt = next;
        if let Some(store) = self.tools.advisor() {
            store.bind_system_prompt(match &self.system_prompt {
                SystemPrompt::Custom(text) => Some(text.clone()),
                _ => None,
            });
        }
        Ok(true)
    }

    fn replace_history_system_prompt(&mut self) -> anyhow::Result<()> {
        let SystemPrompt::Custom(prompt) = &self.system_prompt else {
            return Ok(());
        };
        let mut history = self.sessions.history();
        match history.first() {
            Some(Message::System(_)) => history[0] = Message::System(prompt.clone()),
            _ => history.insert(0, Message::System(prompt.clone())),
        }
        self.sessions.session().replace_history(history)?;
        Ok(())
    }
}
