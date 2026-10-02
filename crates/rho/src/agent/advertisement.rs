//! The catalog information advertised to a delegating parent, not child policy.
//!
//! Only IDs and descriptions reach the parent. Prompt, model, reasoning,
//! runtime, and tools are child policy: each launch reloads them from disk.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{AgentCatalog, DEFAULT_AGENT_ID};

/// Agent IDs and descriptions as a parent model was told about them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AdvertisedAgents(BTreeMap<String, String>);

/// One correction to a previously advertised catalog. Each variant states the
/// new fact outright, so applying a correction never depends on its baseline.
///
/// Persisted in session history and parsed back to rebuild what the model
/// was told. Renaming a tag or field makes older corrections unreadable, which
/// only causes them to be re-sent, but keep the wire shape stable anyway.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub(crate) enum AdvertisedChange {
    Unavailable {
        agent_id: String,
    },
    Available {
        agent_id: String,
        description: String,
    },
    Description {
        agent_id: String,
        previous: String,
        description: String,
    },
}

impl AdvertisedAgents {
    /// Models are deliberately absent: each run reports its own model separately.
    pub(crate) fn from_catalog(catalog: &AgentCatalog) -> Self {
        Self(
            catalog
                .iter()
                .filter(|entry| entry.definition.id.as_str() != DEFAULT_AGENT_ID)
                .map(|entry| {
                    (
                        entry.definition.id.to_string(),
                        entry.definition.description.clone(),
                    )
                })
                .collect(),
        )
    }

    /// `(id, description)` pairs in ID order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(id, description)| (id.as_str(), description.as_str()))
    }

    /// Corrections that turn this advertisement into `next`, in ID order.
    pub(crate) fn changes_to(&self, next: &Self) -> Vec<AdvertisedChange> {
        let ids: BTreeSet<&String> = self.0.keys().chain(next.0.keys()).collect();
        ids.into_iter()
            .filter_map(|id| match (self.0.get(id), next.0.get(id)) {
                (Some(_), None) => Some(AdvertisedChange::Unavailable {
                    agent_id: id.clone(),
                }),
                (None, Some(description)) => Some(AdvertisedChange::Available {
                    agent_id: id.clone(),
                    description: description.clone(),
                }),
                (Some(previous), Some(description)) if previous != description => {
                    Some(AdvertisedChange::Description {
                        agent_id: id.clone(),
                        previous: previous.clone(),
                        description: description.clone(),
                    })
                }
                (Some(_), Some(_)) | (None, None) => None,
            })
            .collect()
    }

    pub(crate) fn apply(&mut self, change: &AdvertisedChange) {
        match change {
            AdvertisedChange::Unavailable { agent_id } => {
                self.0.remove(agent_id);
            }
            AdvertisedChange::Available {
                agent_id,
                description,
            }
            | AdvertisedChange::Description {
                agent_id,
                description,
                ..
            } => {
                self.0.insert(agent_id.clone(), description.clone());
            }
        }
    }
}

#[cfg(test)]
#[path = "advertisement_tests.rs"]
mod tests;
