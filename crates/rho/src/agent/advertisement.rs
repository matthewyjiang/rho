//! The catalog information advertised to a delegating parent, not child policy.

use std::collections::BTreeMap;

use super::AgentCatalog;

/// Keep this surface shared by the startup tool description and later corrections.
/// Models are deliberately absent: each run reports its own model separately.
pub(crate) fn advertised_agents(catalog: &AgentCatalog) -> BTreeMap<String, String> {
    catalog
        .iter()
        .filter(|entry| entry.definition.id.as_str() != "default")
        .map(|entry| {
            (
                entry.definition.id.to_string(),
                entry.definition.description.clone(),
            )
        })
        .collect()
}
