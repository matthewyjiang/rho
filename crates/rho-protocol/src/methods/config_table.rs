//! Structured tables of the agent's `config.toml` read and written entry by
//! entry, so a client can copy chosen rows of them from one daemon to
//! another: provider profiles, model aliases, and favorite models.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ConfigRevision;

/// Read one table's entries, with the revision a [`CONFIG_TABLE_SET`] sends
/// back. A daemon that predates it answers
/// [`crate::ErrorCode::MethodNotFound`].
pub const CONFIG_TABLE_GET: &str = "rho/config/table/get";
/// Put or remove entries of one table in one atomic write. Needs
/// [`crate::ClientCapability::Answer`]: a put can send a provider's saved key
/// to a new `base_url`, which a model driving session control must not do.
pub const CONFIG_TABLE_SET: &str = "rho/config/table/set";

/// A table of `config.toml` that [`CONFIG_TABLE_GET`] and
/// [`CONFIG_TABLE_SET`] edit, spelled as its [`super::FileOnlyTable::key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub enum ConfigTable {
    /// Provider profiles: `ollama` for `[providers.ollama]`, or a custom
    /// provider's name for its `[providers.custom.<name>]`.
    #[serde(rename = "providers")]
    Providers,
    /// `[model.aliases]`, by alias name; each value is the alias's target,
    /// such as `"openai/gpt-5-mini"`.
    #[serde(rename = "model.aliases")]
    ModelAliases,
    /// `model.favorite_models`, by model reference, in the file's order;
    /// each value is `null`.
    #[serde(rename = "model.favorite_models")]
    FavoriteModels,
    /// A value a newer peer sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One entry of a [`ConfigTable`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct TableEntry {
    pub key: String,
    /// The entry as `config.toml` holds it, as JSON: a table is an object.
    pub value: Value,
}

impl TableEntry {
    /// Builds it from each of its fields.
    pub fn new(key: String, value: Value) -> Self {
        Self { key, value }
    }
}

/// One change of a [`CONFIG_TABLE_SET`]. The daemon reads it, so it has no
/// `Unknown`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TableChange {
    /// Write `value` as the whole entry `key`, adding it when the table
    /// lacks it; a field `value` leaves out is removed. A new favorite is
    /// added last, and one already there keeps its place.
    Put { key: String, value: Value },
    /// Remove the entry `key`, which the table must hold.
    Remove { key: String },
}

impl TableChange {
    /// The entry the change is to.
    pub fn key(&self) -> &str {
        match self {
            Self::Put { key, .. } | Self::Remove { key } => key,
        }
    }
}

/// Params of [`CONFIG_TABLE_GET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ConfigTableGetParams {
    pub table: ConfigTable,
}

impl ConfigTableGetParams {
    /// Builds it from each of its fields.
    pub fn new(table: ConfigTable) -> Self {
        Self { table }
    }
}

/// Result of [`CONFIG_TABLE_GET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ConfigTableGetResponse {
    /// Send it with the next [`CONFIG_TABLE_SET`] of this table. It is the
    /// table's own: [`super::CONFIG_GET`]'s revision does not cover it.
    pub revision: ConfigRevision,
    /// Provider profiles and aliases by key; favorites in the file's
    /// order.
    pub entries: Vec<TableEntry>,
}

impl ConfigTableGetResponse {
    /// Builds it from each of its fields.
    pub fn new(revision: ConfigRevision, entries: Vec<TableEntry>) -> Self {
        Self { revision, entries }
    }
}

/// Params of [`CONFIG_TABLE_SET`]. Every change lands, or none does: the
/// whole file must still load with them, and a change the daemon refuses
/// leaves the file as it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ConfigTableSetParams {
    pub table: ConfigTable,
    /// The table's revision the changes were made against. A stale one is
    /// refused with [`crate::ErrorCode::ConfigConflict`].
    pub revision: ConfigRevision,
    /// Applied in order.
    pub changes: Vec<TableChange>,
}

impl ConfigTableSetParams {
    /// Builds it from each of its fields.
    pub fn new(table: ConfigTable, revision: ConfigRevision, changes: Vec<TableChange>) -> Self {
        Self {
            table,
            revision,
            changes,
        }
    }
}

/// Result of [`CONFIG_TABLE_SET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ConfigTableSetResponse {
    /// The table's revision with the changes.
    pub revision: ConfigRevision,
}

impl ConfigTableSetResponse {
    /// Builds it from each of its fields.
    pub fn new(revision: ConfigRevision) -> Self {
        Self { revision }
    }
}
