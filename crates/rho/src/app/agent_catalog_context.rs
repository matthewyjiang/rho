//! Append corrections to the advertised agent catalog without exposing child policy.

use serde::Serialize;

use crate::agent::{advertised_agents, AgentCatalog};

use super::interactive_runtime::InteractiveRuntime;

#[derive(Serialize)]
#[serde(tag = "change", rename_all = "snake_case")]
enum CatalogChange<'a> {
    Unavailable {
        agent_id: &'a str,
    },
    Available {
        agent_id: &'a str,
        description: &'a str,
    },
    Description {
        agent_id: &'a str,
        previous: &'a str,
        description: &'a str,
    },
}

impl InteractiveRuntime {
    /// Only correct fields in the parent-facing tool catalog. Prompt, model,
    /// reasoning, runtime, and tools are child policy and were never advertised.
    /// Returns the display notice only after its model context is durably saved.
    pub(crate) fn append_agent_catalog_changes(
        &mut self,
        before: &AgentCatalog,
        after: &AgentCatalog,
    ) -> anyhow::Result<Option<String>> {
        if !self.tool_specs().iter().any(|spec| spec.name == "agent") {
            return Ok(None);
        }
        let before = advertised_agents(before);
        let after = advertised_agents(after);
        let mut changes = Vec::new();
        for (agent_id, previous) in &before {
            match after.get(agent_id) {
                None => changes.push(CatalogChange::Unavailable { agent_id }),
                Some(description) if description != previous => {
                    changes.push(CatalogChange::Description {
                        agent_id,
                        previous,
                        description,
                    });
                }
                Some(_) => {}
            }
        }
        for (agent_id, description) in &after {
            if !before.contains_key(agent_id) {
                changes.push(CatalogChange::Available {
                    agent_id,
                    description,
                });
            }
        }
        if changes.is_empty() {
            return Ok(None);
        }
        let display = "agent catalog updated for future delegated calls".to_string();
        // Literal system messages may be hoisted into the prefix by providers.
        // Keep the correction at the tail, just like other host-context notices.
        let context = format!(
            "[agent catalog updated]\n{}",
            serde_json::to_string(&changes)?
        );
        self.append_user_context_with_display(context, display.clone())?;
        Ok(Some(display))
    }
}
