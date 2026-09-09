//! Deferred MCP connect and pre-request system prompt refresh.

use futures_util::FutureExt;
use rho_sdk::SystemPrompt;

use super::InteractiveRuntime;
use crate::{model_identity::PromptModel, prompt, tools::mcp::McpConnectOutcome};

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
                        let prompt_changed = self.refresh_startup_system_prompt()?;
                        if prompt_changed {
                            self.replace_history_system_prompt()?;
                        }
                        changed |= prompt_changed;
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
        if let Some(template) = self.prompt_template.as_mut() {
            let mut retained = String::new();
            prompt::append_mcp_instructions(
                &mut retained,
                instructions
                    .iter()
                    .map(|(identity, text)| (identity.as_str(), text.as_str())),
            );
            template.append_retained(&retained);
        }
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
        let Some(template) = self.prompt_template.as_ref() else {
            return Ok(false);
        };
        // Catalog hydration changes display names, not the selected file. Keep
        // startup AGENTS/skills and the loaded model prompt without filesystem IO.
        let running = PromptModel::from_sdk_identity(&self.provider.provider().identity());
        let built = template.render(&running, self.model_prompt.as_ref());
        let next = SystemPrompt::Custom(built.text);
        if next == self.system_prompt {
            return Ok(false);
        }
        self.diagnostics.update_prompt_sources(built.sources);
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
        crate::app::conversation_switch::replace_system_prompt(self.sessions.session(), prompt)?;
        Ok(())
    }
}
