//! The tables `rho/config/table/*` edits entry by entry: provider profiles,
//! model aliases, and favorite models. This module says where each table
//! keeps its entries in `config.toml` and how a put is checked: the entry is
//! decoded with the serde type the loader reads it with, then the whole file
//! is loaded ([`entries::load_edited`]) before anything is saved.

use std::collections::BTreeMap;

use rho_protocol::{ConfigRevision, ConfigTable, TableChange, TableEntry};
use rho_providers::model::favorites::{
    favorite_model_from_value, favorite_model_values, normalized_favorite_models, FavoriteModel,
};
use serde_json::Value as Json;
use toml_edit::{Array, Item, Value};

use super::{
    document::decode_table,
    entries::{self, ChangeError},
    provider_config::PartialEndpointConfig,
    Config, ConfigDocument, ConfigError, ConfigWarning,
};
use crate::model_aliases::ModelAliases;

/// A table `rho/config/table/*` edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditableTable {
    /// `[providers.ollama]` by `ollama`, and each `[providers.custom.<name>]`
    /// by its name.
    Providers,
    /// `[model.aliases]`, each alias's target string by its name.
    ModelAliases,
    /// `model.favorite_models`, each reference in list order, with no value.
    FavoriteModels,
}

const PROVIDERS: &str = "providers";
const OLLAMA: &str = "ollama";
const CUSTOM_PROVIDERS: [&str; 2] = [PROVIDERS, "custom"];
const ALIASES: [&str; 2] = ["model", "aliases"];
const FAVORITES: [&str; 2] = ["model", "favorite_models"];

impl EditableTable {
    /// The table the protocol names, or `None` for one this daemon does not
    /// know.
    pub(crate) fn from_wire(table: ConfigTable) -> Option<Self> {
        match table {
            ConfigTable::Providers => Some(Self::Providers),
            ConfigTable::ModelAliases => Some(Self::ModelAliases),
            ConfigTable::FavoriteModels => Some(Self::FavoriteModels),
            _ => None,
        }
    }

    /// How errors name entry `key`: its key in `config.toml`, or for a
    /// favorite the list and the reference.
    fn label(self, key: &str) -> String {
        match self {
            Self::FavoriteModels => format!("{}[{}]", FAVORITES.join("."), Json::from(key)),
            Self::Providers | Self::ModelAliases => entries::dotted_key(&self.entry_path(key)),
        }
    }

    /// The key path of entry `key`. Favorites are values of one list, which
    /// is their path.
    fn entry_path(self, key: &str) -> Vec<&str> {
        match self {
            Self::Providers if key == OLLAMA => vec![PROVIDERS, OLLAMA],
            Self::Providers => entries::entry_path(&CUSTOM_PROVIDERS, key),
            Self::ModelAliases => entries::entry_path(&ALIASES, key),
            Self::FavoriteModels => FAVORITES.to_vec(),
        }
    }
}

/// The entries of `table` that `doc` holds, and their revision: provider
/// profiles and aliases by key, favorites in list order as the loader reads
/// them.
pub(crate) fn read_table(
    doc: &ConfigDocument,
    table: EditableTable,
) -> (ConfigRevision, Vec<TableEntry>) {
    let entries = table_entries(doc, table);
    (entries::revision(&entries), entries)
}

/// The revision [`read_table`] gives `table` as `doc` holds it.
pub(crate) fn table_revision(doc: &ConfigDocument, table: EditableTable) -> ConfigRevision {
    read_table(doc, table).0
}

fn table_entries(doc: &ConfigDocument, table: EditableTable) -> Vec<TableEntry> {
    let keyed = |entries: BTreeMap<String, Json>| {
        entries
            .into_iter()
            .map(|(key, value)| TableEntry::new(key, value))
            .collect()
    };
    match table {
        EditableTable::Providers => {
            // A custom table named after a built-in provider is shadowed by
            // it, and its name would read as the built-in's own entry, such
            // as `[providers.ollama]`.
            let mut providers = entries::keyed_entries(doc, &CUSTOM_PROVIDERS);
            providers.retain(|name, _| !is_builtin_provider(name));
            if let Some(ollama) = doc.get_at(&[PROVIDERS, OLLAMA]) {
                providers.insert(OLLAMA.to_owned(), entries::to_json(ollama));
            }
            keyed(providers)
        }
        EditableTable::ModelAliases => keyed(entries::keyed_entries(doc, &ALIASES)),
        EditableTable::FavoriteModels => favorite_model_values(&favorites(doc))
            .into_iter()
            .map(|key| TableEntry::new(key, Json::Null))
            .collect(),
    }
}

/// Applies `changes`, in order, to `table` in `doc`. Every change lands or
/// none does: an entry its loader refuses, a remove of an entry the table
/// lacks, or a file that would not load leaves `doc` as it was. Returns the
/// config `doc` now loads as.
pub(crate) fn apply_table_changes(
    doc: &mut ConfigDocument,
    table: EditableTable,
    changes: &[TableChange],
) -> Result<(Config, Vec<ConfigWarning>), ChangeError> {
    let mut edited = doc.clone();
    let mut errors = Vec::new();
    let mut failed = Vec::new();
    for change in changes {
        // A later change to an entry that failed would fail for that reason.
        if failed.contains(&change.key()) {
            continue;
        }
        if let Err(message) = apply(&mut edited, table, change) {
            errors.push(ConfigError::at(table.label(change.key()), message));
            failed.push(change.key());
        }
    }
    if !errors.is_empty() {
        return Err(ChangeError::Refused(errors));
    }
    let changed = changes
        .iter()
        .map(|change| table.entry_path(change.key()))
        .collect::<Vec<_>>();
    let loaded = entries::load_edited(doc, &edited, &changed)?;
    *doc = edited;
    Ok(loaded)
}

fn apply(
    doc: &mut ConfigDocument,
    table: EditableTable,
    change: &TableChange,
) -> Result<(), String> {
    match (table, change) {
        (EditableTable::Providers, TableChange::Put { key, .. } | TableChange::Remove { key })
            if key != OLLAMA && is_builtin_provider(key) =>
        {
            Err(format!(
                "`{key}` is a built-in provider, which only `ollama` has a profile for; name a custom provider something else"
            ))
        }
        (EditableTable::Providers, TableChange::Put { key, value }) => {
            let item = entries::to_item(value)?;
            decode_table::<PartialEndpointConfig>(&item)?;
            doc.set_item_at(&table.entry_path(key), Some(item));
            Ok(())
        }
        (EditableTable::ModelAliases, TableChange::Put { key, value }) => {
            let target = value
                .as_str()
                .ok_or("must be a model reference, such as \"openai/gpt-5-mini\"")?;
            ModelAliases::from_entries(BTreeMap::from([(key.to_owned(), target.to_owned())]))?;
            doc.set_item_at(
                &table.entry_path(key),
                Some(Item::Value(Value::from(target))),
            );
            Ok(())
        }
        (EditableTable::Providers | EditableTable::ModelAliases, TableChange::Remove { key }) => {
            let path = table.entry_path(key);
            doc.get_at(&path).ok_or_else(no_such_entry)?;
            doc.remove_at(&path);
            Ok(())
        }
        (EditableTable::FavoriteModels, TableChange::Put { key, value }) => {
            put_favorite(doc, key, value)
        }
        (EditableTable::FavoriteModels, TableChange::Remove { key }) => remove_favorite(doc, key),
    }
}

/// The favorites the list holds, as the loader reads them: each in its
/// canonical form, in list order, once.
fn favorites(doc: &ConfigDocument) -> Vec<FavoriteModel> {
    let listed = doc
        .get_at(&FAVORITES)
        .and_then(Item::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    normalized_favorite_models(&listed)
}

/// Whether the list value `listed` is `favorite`, however it is spelled.
fn lists(listed: &Value, favorite: &FavoriteModel) -> bool {
    listed
        .as_str()
        .and_then(favorite_model_from_value)
        .is_some_and(|listed| listed.matches(&favorite.provider, &favorite.model))
}

/// Adds `key` last, in its canonical form, unless the list holds it, which
/// keeps its place. The rest of the list stays as written.
fn put_favorite(doc: &mut ConfigDocument, key: &str, value: &Json) -> Result<(), String> {
    if !value.is_null() {
        return Err("a favorite has no value; send null".to_owned());
    }
    let favorite = favorite_model_from_value(key)
        .ok_or("must be a provider/model reference, such as \"openai/gpt-5.5\"")?;
    match doc.get_at_mut(&FAVORITES).and_then(Item::as_array_mut) {
        Some(list) if list.iter().any(|listed| lists(listed, &favorite)) => {}
        Some(list) => list.push(favorite.value()),
        None => {
            let list = Array::from_iter([favorite.value()]);
            doc.set_item_at(&FAVORITES, Some(Item::Value(Value::Array(list))));
        }
    }
    Ok(())
}

/// Removes every copy of `key`, however each is spelled, and the list once
/// it is empty.
fn remove_favorite(doc: &mut ConfigDocument, key: &str) -> Result<(), String> {
    let favorite = favorite_model_from_value(key).ok_or_else(no_such_entry)?;
    let list = doc
        .get_at_mut(&FAVORITES)
        .and_then(Item::as_array_mut)
        .filter(|list| list.iter().any(|listed| lists(listed, &favorite)))
        .ok_or_else(no_such_entry)?;
    list.retain(|listed| !lists(listed, &favorite));
    if list.is_empty() {
        doc.remove_at(&FAVORITES);
    }
    Ok(())
}

/// Whether `name` is a built-in provider's. The providers table lists no
/// custom table by such a name, and takes none.
fn is_builtin_provider(name: &str) -> bool {
    rho_providers::provider::builtin_providers()
        .iter()
        .any(|builtin| builtin.name == name)
}

fn no_such_entry() -> String {
    "no such entry".to_owned()
}

#[cfg(test)]
#[path = "config_tables_tests.rs"]
mod tests;
