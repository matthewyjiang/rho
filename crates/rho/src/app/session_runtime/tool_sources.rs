//! Tool sources a session loads after it opens (see
//! [`crate::app::tools_prompt::ToolSourceLoad::Deferred`]): until they
//! attach, the session's MCP inventory reads `connecting` and its prompts
//! wait at [`SessionRuntime::begin_prompt`].

use std::future::Future;

use rho_sdk::SystemPrompt;

use super::{model_scoped_config, SessionRuntime};
use crate::{
    app::{
        conversation_switch::replace_system_prompt,
        tools_prompt::{DeferredToolSources, PendingToolSources},
    },
    config::Config,
    model_identity::PromptModel,
    tools::mcp::McpConnectOutcome,
};

impl SessionRuntime {
    /// Resolves once the session's deferred tool sources finished loading;
    /// `None` when none are loading. It borrows nothing, so a host can wait
    /// for it without holding the session.
    pub(crate) fn tool_sources_loaded(&self) -> Option<impl Future<Output = ()> + Send + 'static> {
        match self.built.tool_sources.as_ref()? {
            PendingToolSources::Loading(sources) => Some(sources.loaded()),
            PendingToolSources::Unbound => None,
        }
    }

    /// Attaches the deferred tool sources once they finished loading and
    /// rebuilds the runtime with them, and says whether it did. Until they
    /// loaded, and once the runtime runs with them, it does nothing.
    ///
    /// The system prompt is rendered again with them, as opening renders
    /// it when they load inline: a reopened session's saved prompt was
    /// already replaced at opening, and they attach before the first model
    /// request of each opening (prompts wait for them), so no request has
    /// cached the prompt they replace.
    ///
    /// On error the tools stay attached and the runtime keeps the list it
    /// had; the next call tries the rebuild again.
    pub(crate) async fn attach_loaded_tool_sources(
        &mut self,
        process_config: &Config,
    ) -> anyhow::Result<bool> {
        match self.built.tool_sources.take() {
            None => return Ok(false),
            Some(PendingToolSources::Loading(sources)) if !sources.is_loaded() => {
                self.built.tool_sources = Some(PendingToolSources::Loading(sources));
                return Ok(false);
            }
            Some(PendingToolSources::Loading(sources)) => self.attach(sources).await,
            Some(PendingToolSources::Unbound) => {}
        }
        if let Err(error) = self.bind_tool_sources(process_config).await {
            self.built.tool_sources = Some(PendingToolSources::Unbound);
            return Err(error);
        }
        Ok(true)
    }

    /// Puts what `sources` loaded into the session's tool set.
    async fn attach(&mut self, sources: DeferredToolSources) {
        match sources.outcome().await {
            Ok(outcome) => {
                self.built.tools.attach_mcp(outcome.mcp);
                self.built.tools.attach_openapi(outcome.openapi);
            }
            // Say so, or the inventory reads `connecting` until the session ends.
            Err(error) => {
                let mut report = self.built.tools.mcp_report().clone();
                report.fail_connecting(&format!("MCP connect did not finish: {error}"));
                self.built.tools.attach_mcp(McpConnectOutcome {
                    report,
                    bundle: None,
                    catalog: Default::default(),
                });
            }
        }
        for problem in self
            .built
            .tools
            .openapi_report()
            .errors
            .iter()
            .chain(&self.built.tools.openapi_report().warnings)
        {
            tracing::warn!("{problem}");
        }
    }

    /// Renders the system prompt with the tool set's sources and rebuilds
    /// the runtime on the tool set.
    async fn bind_tool_sources(&mut self, process_config: &Config) -> anyhow::Result<()> {
        if let Some(template) = self.built.prompt_template.as_mut() {
            template.replace_tool_sources(
                self.built.tools.mcp_report(),
                self.built.tools.openapi_report(),
            );
            let running = PromptModel::from_sdk_identity(&self.built.provider.identity());
            let built = template.render(&running, self.built.prompt.loaded.as_ref());
            self.built.prompt.adopt(
                crate::app::active_prompt::ActivePrompt::from_prepared(built),
                &self.built.diagnostics,
                self.built.tools.advisor(),
            );
            if let SystemPrompt::Custom(text) = &self.built.prompt.system {
                replace_system_prompt(&self.built.session, text)?;
            }
        }
        let config = model_scoped_config(&self.current_model(), process_config);
        let (mode, writes) = (self.permission.mode, self.permission.writes.clone());
        self.rebuild_runtime(&config, mode, writes).await?;
        Ok(())
    }
}
