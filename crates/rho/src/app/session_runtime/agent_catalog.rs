//! Keeps the model's view of the agents it can delegate to current by
//! appending corrections, never by rewriting the tool schema or history.

use rho_sdk::model::{ContentBlock, Message};

use super::SessionRuntime;
use crate::agent::{AdvertisedAgents, AdvertisedChange, AgentCatalog};

/// Starts the context a correction is recorded as.
pub(in crate::app) const CONTEXT_PREFIX: &str = "[agent catalog updated]\n";
/// The row a correction shows as.
pub(in crate::app) const DISPLAY: &str = "agent catalog updated for future delegated calls";

/// Corrections a recorded notice carried. Matches only the exact single-text
/// shape [`Message::user_text`] writes, so ordinary prompts stay prompts.
pub(in crate::app) fn recorded_changes(message: &Message) -> Option<Vec<AdvertisedChange>> {
    let Message::User(blocks) = message else {
        return None;
    };
    let [ContentBlock::Text(text)] = blocks.as_slice() else {
        return None;
    };
    serde_json::from_str(text.strip_prefix(CONTEXT_PREFIX)?).ok()
}

impl SessionRuntime {
    /// Reads the workspace's agent definitions and appends whatever the
    /// model has not been told about them: the `agent` tool's spec plus
    /// every correction in live history is what it knows, so a correction
    /// that left history is sent again. Child policy (prompt, model, tools)
    /// never appears; launches reload it from disk. Returns the row a
    /// correction saved as, or `None` when the model is current or the
    /// session has no `agent` tool.
    pub(crate) fn sync_agent_catalog(&mut self) -> anyhow::Result<Option<&'static str>> {
        let current = AgentCatalog::discover(self.storage.cwd(), self.project_trust())?;
        self.tell_agent_catalog(&current)
    }

    /// [`Self::sync_agent_catalog`] for the definitions `current` holds.
    fn tell_agent_catalog(
        &mut self,
        current: &AgentCatalog,
    ) -> anyhow::Result<Option<&'static str>> {
        let Some(mut acknowledged) = self.built.tools.advertised_agents().cloned() else {
            return Ok(None);
        };
        for change in self
            .built
            .session
            .history()
            .iter()
            .filter_map(recorded_changes)
            .flatten()
        {
            acknowledged.apply(&change);
        }
        let changes = acknowledged.changes_to(&AdvertisedAgents::from_catalog(current));
        if changes.is_empty() {
            return Ok(None);
        }
        // User-role context stays at the tail; providers may hoist system
        // messages into the cached prefix.
        let context = format!("{CONTEXT_PREFIX}{}", serde_json::to_string(&changes)?);
        self.append_context(context, Message::user_text(DISPLAY), [])?;
        Ok(Some(DISPLAY))
    }
}

#[cfg(test)]
#[path = "agent_catalog_tests.rs"]
mod tests;
