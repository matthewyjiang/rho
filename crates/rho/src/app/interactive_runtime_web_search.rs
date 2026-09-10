//! Search route changes at the idle boundary.
use super::*;

impl InteractiveRuntime {
    /// Apply search configuration atomically between turns. Provider-native search
    /// and the client tool must use the same configuration snapshot.
    pub(crate) async fn apply_web_search(
        &mut self,
        config: Config,
        provider: Arc<dyn rho_sdk::provider::ModelProvider>,
    ) -> anyhow::Result<()> {
        if self.is_session_busy() {
            anyhow::bail!("web search changes apply before the next turn");
        }
        let previous_tool = self.tools.replace_web_search(&config);
        let previous_provider = Arc::clone(self.provider.provider());
        let reasoning = self.provider.reasoning();
        self.provider.adopt(provider, reasoning);
        if let Err(error) = self.rebind_current_session().await {
            self.provider.adopt(previous_provider, reasoning);
            self.tools.restore_web_search(previous_tool);
            return Err(error);
        }
        if let Some(manager) = self.tools.subagents() {
            manager.update_web_search(&config.web_search);
        }
        self.diagnostics.update_web_search(&config.web_search);
        self.config.web_search = config.web_search;
        startup::bind_mcp_sampling(
            &self.mcp_sampling,
            self.provider.provider(),
            self.sessions.session().id(),
            self.workspace.root(),
        );
        self.remember_tool_list();
        Ok(())
    }
}

#[cfg(test)]
#[path = "interactive_runtime_web_search_tests.rs"]
mod tests;
