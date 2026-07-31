//! Hook lifecycle owned by the interactive runtime.
//!
//! The pipeline is started once per session and reattached to every SDK runtime
//! rebuild, so a permission-mode or provider change never starts a second
//! observational worker.

use super::InteractiveRuntime;

impl InteractiveRuntime {
    /// Re-reads hooks for this workspace and reports what is now configured.
    ///
    /// The swap is atomic, so a blocking decision already in flight keeps the
    /// hook set it started with. A session that started without hooks, or without
    /// a whole class of hooks (blocking vs observational), stays that way until
    /// restart: installing a gate or worker means rebuilding the SDK runtime,
    /// which a diagnostics command should not do behind the user's back.
    pub(crate) fn reload_hooks(&self) -> anyhow::Result<crate::hooks::HookReport> {
        let cwd = self.workspace.root();
        let Some(hooks) = self.hooks.as_ref() else {
            let mut discard = |_message: String| {};
            let catalog = crate::hooks::discover_for_cwd(cwd, &mut discard)?;
            anyhow::ensure!(
                catalog.is_empty(),
                "hooks were added since this session started; restart Rho to load them"
            );
            let mut report = crate::hooks::HookReport::disabled();
            report.skipped_untrusted = catalog
                .skipped_untrusted()
                .map(|skipped| crate::paths::display(&skipped.path));
            report.skipped_untrusted_error = catalog
                .skipped_untrusted()
                .and_then(|skipped| skipped.error())
                .map(ToString::to_string);
            report.hooks = crate::hooks::contract_views(&catalog);
            return Ok(report);
        };
        hooks.reload_for_cwd(cwd)?;
        Ok(crate::hooks::HookInspector::new(hooks).report())
    }

    /// Reports the session boundary and lets queued observational hooks finish.
    ///
    /// Only the host knows an interactive session has ended, so `session_completed`
    /// is dispatched here rather than by the SDK.
    pub(super) async fn drain_hooks(&mut self) {
        let Some(hooks) = self.hooks.take() else {
            return;
        };
        self.runtime
            .hooks()
            .session_completed(self.sessions.session().id(), self.completed_runs);
        hooks.shutdown(crate::hooks::DRAIN_GRACE).await;
    }
}
