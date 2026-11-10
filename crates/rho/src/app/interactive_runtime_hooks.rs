//! Hook lifecycle owned by the interactive runtime.
//!
//! The pipeline is started once per session and reattached to every SDK runtime
//! rebuild, so a permission-mode or provider change never starts a second
//! observational worker.

use super::InteractiveRuntime;
use crate::session_environment::Ambient;

impl InteractiveRuntime {
    /// Re-reads hooks for this workspace and reports what is now configured
    /// (see [`crate::hooks::reload_report`]). The interactive host always
    /// runs in this process's environment (#1659).
    pub(crate) fn reload_hooks(&self) -> anyhow::Result<crate::hooks::HookReport> {
        crate::hooks::reload_report(
            self.hooks.as_ref(),
            self.workspace.root(),
            crate::workspace::ProjectTrusts::of_this_process(),
            crate::hooks::running_inside_hook(&Ambient::ThisProcess),
        )
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
