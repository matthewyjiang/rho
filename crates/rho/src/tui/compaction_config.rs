//! Apply saved compaction settings without replacing an in-flight compactor.

use super::{App, InteractiveRuntime};

#[cfg(test)]
#[path = "compaction_config_tests.rs"]
mod tests;

impl App {
    /// Coalesce edits during a run or compact; apply before the next idle check
    /// or provider start, including queued prompts that bypass the outer loop.
    pub(super) fn apply_pending_compaction_config(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !self.compaction_reload_pending || agent.is_session_busy() {
            return Ok(false);
        }
        let saved = self.info.services.config_repository.load()?;
        agent.set_compaction_config(crate::compaction::CompactionConfig::from(&saved))?;
        self.compaction_reload_pending = false;
        Ok(true)
    }
}
