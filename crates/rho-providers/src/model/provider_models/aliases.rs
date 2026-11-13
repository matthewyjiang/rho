//! Model aliases from provider discovery.
//!
//! Some hosts list one model under several ids. Mistral lists
//! `mistral-medium-latest` along with eight other names for the same model,
//! and xAI publishes `aliases` such as `grok-4-latest`. Each listed entry names
//! its alternative ids. [`collapse`] keeps one row per model for the picker and
//! records the rest as aliases. Selection then resolves an alias through
//! [`canonical_model`], so a config that names an alias keeps working after the
//! picker stops listing it.

use std::collections::{BTreeMap, HashMap};

use rusqlite::{params, Transaction};

use super::{ProviderModel, ProviderModelRecord};

/// A model as discovery listed it, with the other ids the host gave for it.
pub(super) struct ListedModel {
    pub(super) model: ProviderModel,
    pub(super) aliases: Vec<String>,
}

/// Merges listed entries that share an id into one record per model.
///
/// Entries join a group when one names another's id (or a shared id) as an
/// alias. Each group keeps one listed entry, ranked first by models.dev
/// metadata (`has_catalog_row`), so reasoning levels resolve, then by its id
/// equalling its display name (Mistral sets `name` to the main id). Ties keep
/// the first listed entry. Every other id becomes an alias of the kept entry.
/// Groups keep their first listed position, so output order is stable.
pub(super) fn collapse(
    listed: Vec<ListedModel>,
    has_catalog_row: impl Fn(&str) -> bool,
) -> Vec<ProviderModelRecord> {
    let mut groups = IdGroups::default();
    for entry in &listed {
        for alias in &entry.aliases {
            groups.union(&entry.model.model, alias);
        }
        groups.find(&entry.model.model);
    }

    let mut members: BTreeMap<usize, Vec<ListedModel>> = BTreeMap::new();
    let mut first_index: HashMap<String, usize> = HashMap::new();
    for (index, entry) in listed.into_iter().enumerate() {
        let root = groups.find(&entry.model.model);
        let group = *first_index.entry(root).or_insert(index);
        members.entry(group).or_default().push(entry);
    }

    members
        .into_values()
        .map(|mut members| {
            let rank = |entry: &ListedModel| {
                (
                    has_catalog_row(&entry.model.model),
                    entry.model.display_name == entry.model.model,
                )
            };
            // `max_by_key` keeps the last maximum; reverse so ties keep the
            // first listed entry.
            let (kept, _) = members
                .iter()
                .enumerate()
                .rev()
                .max_by_key(|(_, entry)| rank(entry))
                .expect("every group has at least one listed entry");
            let kept = members.swap_remove(kept);
            let mut aliases = kept.aliases;
            for other in members {
                aliases.push(other.model.model);
                aliases.extend(other.aliases);
            }
            aliases.retain(|alias| *alias != kept.model.model);
            aliases.sort();
            aliases.dedup();
            ProviderModelRecord {
                model: kept.model,
                raw_json: serde_json::Value::Null,
                aliases,
            }
        })
        .collect()
}

/// Union-find over model ids.
#[derive(Default)]
struct IdGroups {
    parent: HashMap<String, String>,
}

impl IdGroups {
    fn find(&mut self, id: &str) -> String {
        let parent = self
            .parent
            .entry(id.to_owned())
            .or_insert_with(|| id.to_owned())
            .clone();
        if parent == id {
            return parent;
        }
        let root = self.find(&parent);
        self.parent.insert(id.to_owned(), root.clone());
        root
    }

    fn union(&mut self, left: &str, right: &str) {
        let left = self.find(left);
        let right = self.find(right);
        if left != right {
            self.parent.insert(right, left);
        }
    }
}

/// Replaces `provider`'s alias rows inside the refresh transaction.
pub(super) fn replace_in(
    tx: &Transaction<'_>,
    provider: &str,
    records: &[ProviderModelRecord],
) -> rusqlite::Result<()> {
    forget_in(tx, provider)?;
    for record in records {
        for alias in &record.aliases {
            tx.execute(
                "insert or ignore into provider_model_aliases (provider, alias, model)
                 values (?1, ?2, ?3)",
                params![provider, alias, record.model.model],
            )?;
        }
    }
    Ok(())
}

pub(super) fn forget_in(tx: &Transaction<'_>, provider: &str) -> rusqlite::Result<()> {
    tx.execute(
        "delete from provider_model_aliases where provider = ?1",
        params![provider],
    )?;
    Ok(())
}

/// The listed model id that `alias` names on `provider`, if discovery
/// recorded it as an alias.
pub(crate) fn canonical_model(provider: &str, alias: &str) -> Option<String> {
    let alias = super::canonicalize_cached_model_id(provider, alias);
    super::with_provider_models_cache(|connection| {
        connection
            .query_row(
                "select model from provider_model_aliases where provider = ?1 and alias = ?2",
                params![provider, alias],
                |row| row.get(0),
            )
            .ok()
    })
    .ok()
    .flatten()
}

#[cfg(test)]
#[path = "aliases_tests.rs"]
mod tests;
