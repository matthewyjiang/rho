//! Search route changes at the idle boundary.
use super::*;

impl InteractiveRuntime {
    /// Apply search configuration between turns. Rebuilds the chat provider only
    /// when native vs client routing changes. Provider-native search and the
    /// client tool must use the same configuration snapshot.
    pub(crate) async fn apply_web_search(
        &mut self,
        config: Config,
        provider: Option<Arc<dyn rho_sdk::provider::ModelProvider>>,
    ) -> anyhow::Result<()> {
        if self.is_session_busy() {
            anyhow::bail!("web search changes apply before the next turn");
        }
        let previous_hosted = self.hosted_web_search_active();
        let next_hosted = crate::tools::web::hosted_web_search_active(&config);
        let hosted_changed = previous_hosted != next_hosted;
        if hosted_changed && provider.is_none() {
            anyhow::bail!("hosted search route changed without a replacement provider");
        }

        let previous_tool = self.tools.replace_web_search(&config);
        let reasoning = self.provider.reasoning();
        let previous_provider = if hosted_changed {
            let previous = Arc::clone(self.provider.provider());
            self.provider
                .adopt(provider.expect("checked above"), reasoning);
            Some(previous)
        } else {
            None
        };
        if let Err(error) = self.rebind_current_session().await {
            if let Some(previous) = previous_provider {
                self.provider.adopt(previous, reasoning);
            }
            self.tools.restore_web_search(previous_tool);
            if let Err(rebind_error) = self.rebind_current_session().await {
                return Err(anyhow::anyhow!("{error}; rollback failed: {rebind_error}"));
            }
            return Err(error);
        }
        if let Some(manager) = self.tools.subagents() {
            manager.update_web_search(&config.web_search);
        }
        self.diagnostics.update_web_search(&config.web_search);
        self.config.web_search = config.web_search;
        if hosted_changed {
            startup::bind_mcp_sampling(
                &self.mcp_sampling,
                self.provider.provider(),
                self.sessions.session().id(),
                self.workspace.root(),
            );
        }
        self.remember_tool_list();
        Ok(())
    }

    pub(crate) fn hosted_web_search_active(&self) -> bool {
        crate::tools::web::hosted_web_search_active(&self.config)
    }
}

#[cfg(test)]
#[path = "interactive_runtime_web_search_tests.rs"]
mod tests;
