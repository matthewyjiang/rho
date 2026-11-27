//! Saving sealed API keys on the target, and undoing that:
//! `rho/sync/import_keys`, `rho/sync/status`, `rho/sync/revert`, and
//! `rho/sync/discard`.
//!
//! An import names the client's `sync`, one id per Sync press. The target
//! journals the value each import replaced, and a digest of the one it
//! wrote, so the client can later revert the keys it copied. The client
//! keeps no secret: the target restores them. The target keeps one journal,
//! the last sync's, for 7 days after its last write.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::sync::{SealedSecrets, SecretItem};

/// Open a bundle sealed to this daemon and save its keys, journaling what
/// each replaced under the request's `sync`.
pub const SYNC_IMPORT_KEYS: &str = "rho/sync/import_keys";
/// Say, per journaled item, whether its value changed since the sync wrote
/// it.
pub const SYNC_STATUS: &str = "rho/sync/status";
/// Restore the values a sync replaced.
pub const SYNC_REVERT: &str = "rho/sync/revert";
/// Forget a sync's journal, keeping what it wrote.
pub const SYNC_DISCARD: &str = "rho/sync/discard";

/// One Sync press, as the client names it: an opaque string, such as a
/// UUID, the client picks.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct SyncId(String);

impl SyncId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SyncId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Something a sync changed that the target can revert. Protocol minors
/// may add kinds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SyncItem {
    /// The saved API key of auth profile `auth`. Built with
    /// [`SyncItem::api_key`].
    #[non_exhaustive]
    ApiKey { auth: String },
    /// The whole MCP server `identity` under `[mcp.servers]`, its secret
    /// values included, as [`super::MCP_CONFIG_SET`] wrote it. Built with
    /// [`SyncItem::mcp_server`].
    #[non_exhaustive]
    McpServer { identity: String },
    /// A kind a newer peer sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl SyncItem {
    /// The saved API key of auth profile `auth`.
    pub fn api_key(auth: impl Into<String>) -> Self {
        Self::ApiKey { auth: auth.into() }
    }

    /// The MCP server `identity`.
    pub fn mcp_server(identity: impl Into<String>) -> Self {
        Self::McpServer {
            identity: identity.into(),
        }
    }
}

/// Params of [`SYNC_IMPORT_KEYS`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncImportKeysParams {
    /// API keys sealed to a recipient this daemon issued, opened as
    /// [`super::SYNC_COMPARE`] opens them, using the recipient up.
    pub sealed: SealedSecrets,
    /// The sync to journal under. One naming another sync than the journal
    /// holds replaces the journal, and the earlier sync can no longer be
    /// reverted.
    pub sync: SyncId,
}

impl SyncImportKeysParams {
    /// Builds it from each of its fields.
    pub fn new(sealed: SealedSecrets, sync: SyncId) -> Self {
        Self { sealed, sync }
    }
}

/// Result of [`SYNC_IMPORT_KEYS`]: each sealed item, in order, with what
/// became of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncImportKeysResponse {
    pub items: Vec<ImportedSecret>,
}

impl SyncImportKeysResponse {
    /// Builds it from each of its fields.
    pub fn new(items: Vec<ImportedSecret>) -> Self {
        Self { items }
    }
}

/// What became of one sealed item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ImportedSecret {
    pub item: SecretItem,
    pub outcome: ImportOutcome,
}

impl ImportedSecret {
    /// Builds it from each of its fields.
    pub fn new(item: SecretItem, outcome: ImportOutcome) -> Self {
        Self { item, outcome }
    }
}

/// What an import did with one item. One item failing does not stop the
/// rest. Protocol minors may add outcomes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ImportOutcome {
    /// Saved, and journaled.
    Saved,
    /// Not saved, for the reason `message` gives, such as an auth profile
    /// the target does not know.
    Failed { message: String },
    /// An outcome a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Params of [`SYNC_STATUS`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncStatusParams {
    /// A sync the journal holds; another is
    /// [`crate::ErrorCode::SyncNotFound`].
    pub sync: SyncId,
}

impl SyncStatusParams {
    /// Builds it from each of its fields.
    pub fn new(sync: SyncId) -> Self {
        Self { sync }
    }
}

/// Result of [`SYNC_STATUS`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncStatusResponse {
    /// Unix time, in seconds, when the journal expires: 7 days after the
    /// sync's last write.
    pub expires_at: u64,
    /// Each item the sync changed and that is not reverted yet.
    pub items: Vec<SyncItemStatus>,
}

impl SyncStatusResponse {
    /// Builds it from each of its fields.
    pub fn new(expires_at: u64, items: Vec<SyncItemStatus>) -> Self {
        Self { expires_at, items }
    }
}

/// Whether one journaled item changed since the sync wrote it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncItemStatus {
    pub item: SyncItem,
    pub state: SyncItemState,
}

impl SyncItemStatus {
    /// Builds it from each of its fields.
    pub fn new(item: SyncItem, state: SyncItemState) -> Self {
        Self { item, state }
    }
}

/// Protocol minors may add states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SyncItemState {
    /// The target holds what the sync wrote.
    Unchanged,
    /// Something changed or removed it since.
    Changed,
    /// A state a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Params of [`SYNC_REVERT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncRevertParams {
    /// A sync the journal holds; another is
    /// [`crate::ErrorCode::SyncNotFound`].
    pub sync: SyncId,
    /// The items to restore. One that changed since the sync is skipped
    /// unless `overwrite` names it too.
    pub items: Vec<SyncItem>,
    /// Items to restore even though they changed since the sync.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overwrite: Vec<SyncItem>,
}

impl SyncRevertParams {
    /// Reverts `items` of `sync`, overwriting none that changed.
    pub fn new(sync: SyncId, items: Vec<SyncItem>) -> Self {
        Self {
            sync,
            items,
            overwrite: Vec::new(),
        }
    }

    /// Restores `overwrite` even where it changed since the sync.
    pub fn with_overwrite(mut self, overwrite: Vec<SyncItem>) -> Self {
        self.overwrite = overwrite;
        self
    }
}

/// Result of [`SYNC_REVERT`]: each requested item, in order, with what
/// became of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncRevertResponse {
    pub items: Vec<RevertedItem>,
}

impl SyncRevertResponse {
    /// Builds it from each of its fields.
    pub fn new(items: Vec<RevertedItem>) -> Self {
        Self { items }
    }
}

/// What a revert did with one item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct RevertedItem {
    pub item: SyncItem,
    pub outcome: RevertOutcome,
}

impl RevertedItem {
    /// Builds it from each of its fields.
    pub fn new(item: SyncItem, outcome: RevertOutcome) -> Self {
        Self { item, outcome }
    }
}

/// What a revert did with one item. One item failing does not stop the
/// rest. Protocol minors may add outcomes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RevertOutcome {
    /// The value from before the sync is back, or, for an item the sync
    /// added, the item is gone. It leaves the journal.
    Restored,
    /// It changed since the sync and was not in `overwrite`, so it was left
    /// alone and stays in the journal.
    Changed,
    /// Not restored, for the reason `message` gives, such as an item the
    /// sync did not change or that was already reverted.
    Failed { message: String },
    /// An outcome a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Params of [`SYNC_DISCARD`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncDiscardParams {
    /// A sync the journal holds; another is
    /// [`crate::ErrorCode::SyncNotFound`].
    pub sync: SyncId,
}

impl SyncDiscardParams {
    /// Builds it from each of its fields.
    pub fn new(sync: SyncId) -> Self {
        Self { sync }
    }
}

/// Result of [`SYNC_DISCARD`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncDiscardResponse {}

#[cfg(test)]
#[path = "sync_journal_tests.rs"]
mod tests;
