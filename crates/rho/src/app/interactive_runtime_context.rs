//! Host presentation and diagnostics use the SDK's session-owned accounting.
use rho_sdk::{model::ContextUsage, ContextEstimate};

use super::InteractiveRuntime;
use crate::diagnostics::CompactionContext;

impl InteractiveRuntime {
    pub(super) fn context_usage(&self, estimate: ContextEstimate) -> ContextUsage {
        // This remains an estimate: the request baseline can be provider-reported,
        // but assistant output and later input have only local token estimates.
        // Use the configured window, exactly as the installed compaction policy
        // does. Provider window reports are available separately in diagnostics.
        ContextUsage::estimated(
            estimate.tokens(),
            self.context_window.or(estimate.reported_context_window()),
        )
    }

    pub(super) fn record_context_estimate(&self, estimate: ContextEstimate) {
        self.diagnostics
            .record_context(self.context_usage(estimate));
        self.diagnostics.record_compaction_context(
            CompactionContext::new(estimate, self.context_window, &self.compaction),
            self.sessions.session().last_compaction_decision(),
        );
    }

    pub(super) fn refresh_context_usage(&mut self) {
        let estimate = self.sessions.session().context_estimate();
        self.runs.note_context_usage(self.context_usage(estimate));
        self.record_context_estimate(estimate);
    }
}
