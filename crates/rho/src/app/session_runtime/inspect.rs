//! Reports a host reads about a session between its turns.

use super::{context_window, SessionRuntime};
use crate::{
    app::context_report::{mcp_tool_servers, ContextReport},
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
