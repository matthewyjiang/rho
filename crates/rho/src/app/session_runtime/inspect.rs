//! Reports a host reads about a session between its turns.

use super::{context_window, model_scoped_config, SessionRuntime};
use crate::{
    app::{
        compaction_report::CompactionReport,
        context_report::{mcp_tool_servers, ContextReport},
        provider_context_report::ProviderContextReport,
        session_options::{resolve_model, AuthPick},
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

    /// What of the conversation the session's model cannot be sent, whether
    /// a `/compact` under `process_config`'s compaction settings would
    /// summarize any of it, and whether the session may switch to the model
    /// that produced it on one of `auths`. Scans the committed history, so
    /// hosts read it on demand.
    pub(crate) fn provider_context_report(
        &self,
        process_config: &Config,
        auths: &[String],
    ) -> ProviderContextReport {
        let session = &self.built.session;
        let history = session.history();
        let config = model_scoped_config(&self.current_model(), process_config);
        let target_tokens = CompactionConfig::from(&config).target_tokens_for_context(
            self.configured_context_window(),
            rho_sdk::CompactionTrigger::Manual,
            session.estimate_context(&history),
        );
        let can_compact = crate::compaction::partition_messages_for_compaction(
            &history,
            &self.built.tools.specs(),
            target_tokens,
        )
        .is_some();
        let current = self.current_model();
        ProviderContextReport::new(
            &history,
            self.built.provider.identity(),
            can_compact,
            |source| {
                resolve_model(
                    &source.provider,
                    &source.model,
                    &current,
                    AuthPick::AnyOf(auths),
                )
                .is_ok()
            },
        )
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
