//! Keep the model's view of the delegable agent catalog current by appending
//! corrections, never by rewriting the tool schema or prior history.

use crate::{
    agent::{AdvertisedAgents, AgentCatalog},
    app::session_runtime::agent_catalog::{recorded_changes, CONTEXT_PREFIX, DISPLAY},
};

use super::InteractiveRuntime;

impl InteractiveRuntime {
    /// The catalog as the model last saw it: the `agent` tool spec plus every
    /// correction in live history. Derived rather than stored, so resume,
    /// compaction, and failed appends need no separate bookkeeping; a
    /// correction that left history is simply sent again.
    fn acknowledged_agents(&self) -> Option<AdvertisedAgents> {
        let mut advertised = self.tools.advertised_agents()?.clone();
        for change in self
            .sessions
            .history()
            .iter()
            .filter_map(recorded_changes)
            .flatten()
        {
            advertised.apply(&change);
        }
        Some(advertised)
    }

    /// Appends whatever the model has not been told about `current`, including
    /// external edits since the last correction. Child policy (prompt, model,
    /// tools, ...) never appears: launches reload it from disk on their own.
    /// Returns transcript text once a correction was durably appended.
    pub(crate) fn sync_agent_catalog(
        &mut self,
        current: &AgentCatalog,
    ) -> anyhow::Result<Option<&'static str>> {
        let Some(acknowledged) = self.acknowledged_agents() else {
            return Ok(None);
        };
        let changes = acknowledged.changes_to(&AdvertisedAgents::from_catalog(current));
        if changes.is_empty() {
            return Ok(None);
        }
        // User-role context stays at the tail; providers may hoist system
        // messages into the cached prefix.
        let context = format!("{CONTEXT_PREFIX}{}", serde_json::to_string(&changes)?);
        self.append_user_context_with_display(context, DISPLAY.into())?;
        Ok(Some(DISPLAY))
    }
}

#[cfg(test)]
#[path = "interactive_runtime_agent_catalog_tests.rs"]
mod tests;
