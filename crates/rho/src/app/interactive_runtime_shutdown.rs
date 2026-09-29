//! Session teardown for the interactive runtime.
//!
//! Quit ends here. An open re-read window is written before the process can
//! exit, because the usual compaction save is `spawn_blocking` and may not run.

use super::super::interactive_state::{
    active_run_disposition, ActiveRunCommand, ActiveRunDisposition,
};
use super::InteractiveRuntime;

impl InteractiveRuntime {
    pub(crate) async fn shutdown(&mut self) {
        if self.runs.is_active() {
            debug_assert_eq!(
                active_run_disposition(ActiveRunCommand::Quit),
                ActiveRunDisposition::CancelAndWait
            );
            self.cancel();
            let _ = self.finish_run().await;
        }
        // The open re-read window lives only in memory. Write it before
        // teardown returns: the usual save is `spawn_blocking` and may not run
        // before exit.
        self.diagnostics.flush_unfinished_compaction();
        // Release the model before the sessions close, so a late server request
        // finds nothing bound rather than a provider on its way out.
        self.mcp_sampling.unbind();
        self.runtime.shutdown();
        self.drain_hooks().await;
        self.tools.shutdown().await;
    }
}
