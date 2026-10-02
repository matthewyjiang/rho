//! Deferred MCP connect and pre-request system prompt refresh.

#[cfg(test)]
#[path = "interactive_runtime_mcp_tests.rs"]
mod tests;

use futures_util::FutureExt;
use rho_sdk::SystemPrompt;

use super::InteractiveRuntime;
use crate::{model_identity::PromptModel, prompt, tools::mcp::McpConnectOutcome};

enum StartupPromptRefresh {
    Rewritten,
    Unchanged,
    Frozen,
}

impl InteractiveRuntime {
    pub(crate) fn mcp_connect_pending(&self) -> bool {
        self.pending_mcp.is_some()
    }

    pub(crate) fn startup_hydrate_pending(&self) -> bool {
        self.pending_mcp.is_some() || self.pending_catalog_names.is_some()
    }

    /// Whether a startup hydrate finished and is waiting for
    /// [`Self::poll_startup_hydrates`]. Unlike [`Self::startup_hydrate_pending`],
    /// this stays false while the work is still in flight, so the UI only
    /// redraws when there is something to apply.
    pub(crate) fn startup_hydrate_ready(&self) -> bool {
        self.pending_mcp
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
            || self
                .pending_catalog_names
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
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
                        match self.refresh_startup_system_prompt() {
                            StartupPromptRefresh::Rewritten => {
                                self.replace_history_system_prompt()?;
                                changed = true;
                            }
                            StartupPromptRefresh::Unchanged | StartupPromptRefresh::Frozen => {}
                        }
                    }
                }
            }
        }
        Ok(changed)
    }

    async fn apply_mcp_connect(&mut self, outcome: McpConnectOutcome) -> anyhow::Result<()> {
        self.tools.attach_mcp(outcome);
        self.mcp_report = self.tools.mcp_report().clone();
        if let Some(template) = self.prompt_template.as_mut() {
            template.replace_mcp(&self.mcp_report);
        }
        let refresh = self.refresh_startup_system_prompt();
        self.rebind_current_session().await?;
        self.remember_tool_list();
        match refresh {
            StartupPromptRefresh::Rewritten => self.replace_history_system_prompt()?,
            StartupPromptRefresh::Unchanged => {}
            StartupPromptRefresh::Frozen => {
                let notice = prompt::mcp_context(&self.mcp_report);
                if !notice.is_empty() {
                    self.append_user_context_with_display(notice.clone(), notice)?;
                }
            }
        }
        Ok(())
    }

    fn refresh_startup_system_prompt(&mut self) -> StartupPromptRefresh {
        if !self.may_rewrite_startup_prompt || self.live_context_warm {
            return StartupPromptRefresh::Frozen;
        }
        let Some(template) = self.prompt_template.as_ref() else {
            return StartupPromptRefresh::Unchanged;
        };
        // Catalog hydration changes display names, not the selected file. Keep
        // startup AGENTS/skills and the loaded model prompt without filesystem IO.
        let running = PromptModel::from_sdk_identity(&self.provider.provider().identity());
        let built = template.render(&running, self.sessions.prompt.loaded.as_ref());
        if self.sessions.prompt.system == SystemPrompt::Custom(built.text.clone()) {
            return StartupPromptRefresh::Unchanged;
        }
        // Hydration retains its startup lifetime until an explicit transition.
        self.sessions.prompt.adopt(
            crate::app::active_prompt::ActivePrompt::from_prepared(built),
            &self.diagnostics,
            self.tools.advisor(),
        );
        StartupPromptRefresh::Rewritten
    }

    fn replace_history_system_prompt(&mut self) -> anyhow::Result<()> {
        let SystemPrompt::Custom(prompt) = &self.sessions.prompt.system else {
            return Ok(());
        };
        crate::app::conversation_switch::replace_system_prompt(self.sessions.session(), prompt)?;
        Ok(())
    }
}
