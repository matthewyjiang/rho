//! The MCP servers of the agent's `config.toml` read and written entry by
//! entry, so a client can copy chosen servers from one daemon to another:
//! `rho/mcp/config/*`.
//!
//! A server's header and environment values are secrets. A read names them
//! and never returns them; a write fills each one from the value the target
//! already holds or from a bundle sealed to it ([`super::SYNC_RECIPIENT`]),
//! so the client never holds one.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ConfigRevision, SealedSecrets, SyncId};

/// Read the servers of `[mcp.servers]`, with the revision an
/// [`MCP_CONFIG_SET`] sends back. A daemon that predates it answers
/// [`crate::ErrorCode::MethodNotFound`].
pub const MCP_CONFIG_GET: &str = "rho/mcp/config/get";
/// Put or remove servers of `[mcp.servers]` in one atomic write. Needs
/// capability `answer`: a put can keep a server's secret while pointing
/// the server elsewhere.
pub const MCP_CONFIG_SET: &str = "rho/mcp/config/set";

/// Which map of a server a secret sits in. Protocol minors may add kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpSecretKind {
    /// `headers`, sent with each request to a Streamable HTTP server.
    Header,
    /// `env`, set for a stdio server's process.
    Env,
    /// A kind a newer peer sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One secret of a server, named without its value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct McpSecretSlot {
    pub kind: McpSecretKind,
    /// The header's or variable's name.
    pub name: String,
}

impl McpSecretSlot {
    /// Builds it from each of its fields.
    pub fn new(kind: McpSecretKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
        }
    }
}

/// One server of `[mcp.servers]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct McpServerEntry {
    /// Its key under `[mcp.servers]`.
    pub identity: String,
    /// The entry as `config.toml` holds it, as JSON, without its `headers`
    /// and `env` maps, whose values are secrets. Everything else stays,
    /// such as `headers_from_env`, which names variables only.
    pub value: Value,
    /// The names in its `headers` and `env` maps.
    pub secrets: Vec<McpSecretSlot>,
    /// Why the daemon does not load it, as [`super::MCP_LIST`] reports; its
    /// `value` is still returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl McpServerEntry {
    /// Builds it from each of its fields.
    pub fn new(
        identity: String,
        value: Value,
        secrets: Vec<McpSecretSlot>,
        error: Option<String>,
    ) -> Self {
        Self {
            identity,
            value,
            secrets,
            error,
        }
    }
}

/// Where a put takes one secret's value from. The daemon reads it, so it
/// has no `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecretSource {
    /// The value the target's entry already holds in that slot.
    Keep,
    /// The value of the matching item of the set's `sealed` bundle:
    /// [`super::SecretItem::McpHeader`] or [`super::SecretItem::McpEnv`]
    /// with this server's identity.
    Sealed,
}

/// One secret a put writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct McpSecret {
    pub slot: McpSecretSlot,
    pub source: SecretSource,
}

impl McpSecret {
    /// Builds it from each of its fields.
    pub fn new(slot: McpSecretSlot, source: SecretSource) -> Self {
        Self { slot, source }
    }
}

/// One change of an [`MCP_CONFIG_SET`]. The daemon reads it, so it has no
/// `Unknown`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum McpServerChange {
    /// Write the whole server `identity`, adding it when the file lacks it:
    /// `value`, which must not hold `headers` or `env`, with those maps
    /// rebuilt from `secrets`. A field or secret it leaves out is removed.
    Put {
        identity: String,
        value: Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        secrets: Vec<McpSecret>,
    },
    /// Remove the server `identity`, which the file must hold.
    Remove { identity: String },
}

impl McpServerChange {
    /// The server the change is to.
    pub fn identity(&self) -> &str {
        match self {
            Self::Put { identity, .. } | Self::Remove { identity } => identity,
        }
    }
}

/// Params of [`MCP_CONFIG_GET`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct McpConfigGetParams {}

/// Result of [`MCP_CONFIG_GET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct McpConfigGetResponse {
    /// Send it with the next [`MCP_CONFIG_SET`]. It covers the secret
    /// values too, under a key the daemon draws when it starts, so it
    /// tells nothing about them, and a daemon restart makes it stale.
    pub revision: ConfigRevision,
    /// By identity.
    pub servers: Vec<McpServerEntry>,
}

impl McpConfigGetResponse {
    /// Builds it from each of its fields.
    pub fn new(revision: ConfigRevision, servers: Vec<McpServerEntry>) -> Self {
        Self { revision, servers }
    }
}

/// Params of [`MCP_CONFIG_SET`]. Every change lands, or none does: the
/// whole file must still load with them, and a change the daemon refuses
/// leaves the file as it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct McpConfigSetParams {
    /// The revision the changes were made against. A stale one is refused
    /// with [`crate::ErrorCode::ConfigConflict`].
    pub revision: ConfigRevision,
    /// Applied in order.
    pub changes: Vec<McpServerChange>,
    /// The values of the secrets whose source is
    /// [`SecretSource::Sealed`], sealed to a recipient this daemon issued.
    /// Its items must be exactly those secrets. Opening it, once the
    /// revision holds, uses the recipient up, as [`super::SYNC_COMPARE`]
    /// does, so a set refused after that needs a new recipient and export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<SealedSecrets>,
    /// Journal what the changes replace under this sync, so
    /// [`super::SYNC_REVERT`] can restore each server. Without it the
    /// write cannot be reverted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncId>,
}

impl McpConfigSetParams {
    /// Changes with no sealed values, not journaled.
    pub fn new(revision: ConfigRevision, changes: Vec<McpServerChange>) -> Self {
        Self {
            revision,
            changes,
            sealed: None,
            sync: None,
        }
    }

    /// These params, filling the sealed secrets from `sealed`.
    pub fn with_sealed(self, sealed: SealedSecrets) -> Self {
        Self {
            sealed: Some(sealed),
            ..self
        }
    }

    /// These params, journaled under `sync`.
    pub fn with_sync(self, sync: SyncId) -> Self {
        Self {
            sync: Some(sync),
            ..self
        }
    }
}

/// Result of [`MCP_CONFIG_SET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct McpConfigSetResponse {
    /// The revision with the changes.
    pub revision: ConfigRevision,
}

impl McpConfigSetResponse {
    /// Builds it from each of its fields.
    pub fn new(revision: ConfigRevision) -> Self {
        Self { revision }
    }
}

#[cfg(test)]
#[path = "mcp_config_tests.rs"]
mod tests;
