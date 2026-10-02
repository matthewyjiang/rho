//! Keep the model's view of the delegable agent catalog current by appending
//! corrections, never by rewriting the tool schema or prior history.

use rho_sdk::model::{ContentBlock, Message};

use crate::agent::{AdvertisedAgents, AdvertisedChange, AgentCatalog};

use super::InteractiveRuntime;

const CONTEXT_PREFIX: &str = "[agent catalog updated]\n";
const DISPLAY: &str = "agent catalog updated for future delegated calls";

/// Corrections a recorded notice carried. Matches only the exact single-text
/// shape [`Message::user_text`] writes, so ordinary prompts stay prompts.
fn recorded_changes(message: &Message) -> Option<Vec<AdvertisedChange>> {
    let Message::User(blocks) = message else {
        return None;
    };
    let [ContentBlock::Text(text)] = blocks.as_slice() else {
        return None;
    };
    serde_json::from_str(text.strip_prefix(CONTEXT_PREFIX)?).ok()
}

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
