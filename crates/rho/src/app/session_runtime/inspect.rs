//! Reports a host reads about a session between its turns.

use super::{context_window, model_scoped_config, SessionRuntime};
use crate::{
    app::{
        compaction_report::CompactionReport,
        context_report::{mcp_tool_servers, ContextReport},
    },
    compaction::CompactionConfig,
    config::Config,
    diagnostics::{CompactionContext, CompactionDiagnostics},
    hooks::HookReport,
};

impl SessionRuntime {
    /// What fills the context window on the next request, by source. Scans
    /// the committed history, so hosts read it on demand.
    pub(crate) fn context_report(&self) -> ContextReport {
        let breakdown = self.built.session.context_breakdown();
        ContextReport::new(
            &breakdown,
            &self.built.diagnostics.prompt_sources(),
            &mcp_tool_servers(self.tools().mcp_report()),
            context_window(self.configured_context_window(), breakdown.estimate()),
        )
    }

    /// The session's compaction settings against its context, and what its
    /// compactions did, under `process_config`'s compaction settings.
    pub(crate) fn compaction_report(&self, process_config: &Config) -> CompactionReport {
        let session = &self.built.session;
        let config = model_scoped_config(&self.current_model(), process_config);
        CompactionReport::new(&CompactionDiagnostics {
            completed: session.compaction_state(),
            current: CompactionContext::new(
                session.context_estimate(),
                self.configured_context_window(),
                &CompactionConfig::from(&config),
            ),
            // Only the in-process TUI checks while idle.
            last_idle_check: None,
            last_provider_check: session.last_compaction_decision().map(Into::into),
            last_compaction: self.built.diagnostics.last_compaction(),
        })
    }

    /// Reads the session's hooks again from their files and reports them
    /// (see [`crate::hooks::reload_report`]).
    pub(crate) fn reload_hooks(&self) -> anyhow::Result<HookReport> {
        let environment = &self.build.host.environment;
        crate::hooks::reload_report(
            self.built.hooks.as_ref(),
            self.cwd(),
            environment.project_trust(),
            crate::hooks::running_inside_hook(&environment.current()),
        )
    }
}
