//! Copying saved secrets from one daemon to another without the client
//! that relays them seeing them: `rho/sync/*`.
//!
//! A client copying settings from a source daemon to a target asks the
//! target for a one-time public key ([`SYNC_RECIPIENT`]), has the source
//! seal the chosen secrets to it ([`SYNC_EXPORT`]), and hands the sealed
//! bundle back to the target, which opens it once: to compare the values
//! with its own ([`SYNC_COMPARE`]), or to save them. The client only ever
//! holds ciphertext. Daemons never talk to each other.
//!
//! Sealing is HPKE (RFC 9180) in base mode with the suite
//! [`SealSuite::X25519HkdfSha256ChaCha20Poly1305`], `info`
//! `"rho sync secrets v1"`, and as `aad` the JSON of the bundle's `items`,
//! so the list the bundle shows in the clear cannot be altered. The
//! plaintext is the JSON array of the items' values, in `items` order.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Issue a one-time public key that secrets can be sealed to. The daemon
/// keeps its private key in memory only, until the first request that opens
/// a bundle sealed to it, five minutes, or the daemon's restart, whichever
/// comes first.
pub const SYNC_RECIPIENT: &str = "rho/sync/recipient";
/// Seal saved secrets to another daemon's [`SYNC_RECIPIENT`] key. It
/// returns secret material, so it needs capability `answer`
/// ([`crate::ClientCapability::Answer`]): anyone holding the recipient's
/// private key can read what it returns.
pub const SYNC_EXPORT: &str = "rho/sync/export";
/// Open a bundle sealed to this daemon and say, per item, whether its value
/// matches the one this daemon holds. It needs capability `answer`, since
/// it answers whether a guessed value is right.
pub const SYNC_COMPARE: &str = "rho/sync/compare";

/// How a bundle is sealed. Protocol minors may add suites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub enum SealSuite {
    /// HPKE with DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, and
    /// ChaCha20-Poly1305.
    #[serde(rename = "x25519-hkdf-sha256-chacha20poly1305")]
    X25519HkdfSha256ChaCha20Poly1305,
    /// A suite a newer peer sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One secret a bundle carries, named without its value. Protocol minors
/// may add kinds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SecretItem {
    /// The saved API key of `auth`, an auth profile of kind
    /// [`crate::AuthKind::ApiKey`], as [`super::AUTH_KEY`] takes it. A key
    /// held only in the daemon's environment is not saved, so it is not
    /// exported. Interactive logins are never exported: copying a refresh
    /// token the provider rotates can sign both machines out. Built with
    /// [`SecretItem::api_key`].
    #[non_exhaustive]
    ApiKey { auth: String },
    /// A kind a newer peer sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl SecretItem {
    /// The saved API key of auth profile `auth`.
    pub fn api_key(auth: impl Into<String>) -> Self {
        Self::ApiKey { auth: auth.into() }
    }
}

/// A one-time public key that secrets can be sealed to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SealRecipient {
    pub suite: SealSuite,
    /// The public key, standard base64.
    pub public_key: String,
    /// Unix time, in seconds, after which the daemon has forgotten the key.
    pub expires_at: u64,
}

impl SealRecipient {
    /// Builds it from each of its fields.
    pub fn new(suite: SealSuite, public_key: String, expires_at: u64) -> Self {
        Self {
            suite,
            public_key,
            expires_at,
        }
    }
}

/// Secrets sealed to one [`SealRecipient`]. Only the daemon that issued the
/// recipient can open it, and only once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SealedSecrets {
    pub suite: SealSuite,
    /// The [`SealRecipient::public_key`] it is sealed to.
    pub recipient: String,
    /// What it carries, in the order of the sealed values.
    pub items: Vec<SecretItem>,
    /// HPKE's encapsulated key, standard base64.
    pub encapped_key: String,
    /// The sealed values with their tag, standard base64.
    pub ciphertext: String,
}

impl SealedSecrets {
    /// Builds it from each of its fields.
    pub fn new(
        suite: SealSuite,
        recipient: String,
        items: Vec<SecretItem>,
        encapped_key: String,
        ciphertext: String,
    ) -> Self {
        Self {
            suite,
            recipient,
            items,
            encapped_key,
            ciphertext,
        }
    }
}

/// Params of [`SYNC_RECIPIENT`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncRecipientParams {}

/// Result of [`SYNC_RECIPIENT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncRecipientResponse {
    pub recipient: SealRecipient,
}

impl SyncRecipientResponse {
    /// Builds it from each of its fields.
    pub fn new(recipient: SealRecipient) -> Self {
        Self { recipient }
    }
}

/// Params of [`SYNC_EXPORT`]. The secrets come from the daemon's own
/// credential store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncExportParams {
    /// The key another daemon issued with [`SYNC_RECIPIENT`]. The exporting
    /// daemon cannot tell who issued it: the client vouches for it.
    pub recipient: SealRecipient,
    /// The secrets to seal. An item of a kind the daemon does not know, or
    /// an auth profile that does not take an API key, is refused with
    /// [`crate::ErrorCode::InvalidParams`].
    pub items: Vec<SecretItem>,
}

impl SyncExportParams {
    /// Builds it from each of its fields.
    pub fn new(recipient: SealRecipient, items: Vec<SecretItem>) -> Self {
        Self { recipient, items }
    }
}

/// Result of [`SYNC_EXPORT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncExportResponse {
    /// The items that have a value, sealed.
    pub sealed: SealedSecrets,
    /// The items that have none saved, left out of `sealed`.
    pub missing: Vec<SecretItem>,
}

impl SyncExportResponse {
    /// Builds it from each of its fields.
    pub fn new(sealed: SealedSecrets, missing: Vec<SecretItem>) -> Self {
        Self { sealed, missing }
    }
}

/// Params of [`SYNC_COMPARE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncCompareParams {
    /// A bundle sealed to a recipient this daemon issued. A recipient it no
    /// longer holds is [`crate::ErrorCode::SealRecipientGone`]; a bundle
    /// that does not open is [`crate::ErrorCode::InvalidParams`], and the
    /// recipient is used up either way. A suite or item kind this daemon
    /// does not know is refused with `InvalidParams` before opening, which
    /// keeps the recipient.
    pub sealed: SealedSecrets,
}

impl SyncCompareParams {
    /// Builds it from each of its fields.
    pub fn new(sealed: SealedSecrets) -> Self {
        Self { sealed }
    }
}

/// Result of [`SYNC_COMPARE`]: each sealed item, in order, with how it
/// compares.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SyncCompareResponse {
    pub items: Vec<SecretComparison>,
}

impl SyncCompareResponse {
    /// Builds it from each of its fields.
    pub fn new(items: Vec<SecretComparison>) -> Self {
        Self { items }
    }
}

/// How one sealed item compares with this daemon's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SecretComparison {
    pub item: SecretItem,
    pub state: CompareState,
}

impl SecretComparison {
    /// Builds it from each of its fields.
    pub fn new(item: SecretItem, state: CompareState) -> Self {
        Self { item, state }
    }
}

/// How a sealed value compares with the one this daemon holds. Protocol
/// minors may add states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompareState {
    /// This daemon holds the same value.
    Same,
    /// This daemon holds another value.
    Differs,
    /// This daemon holds no value for it, or does not know its auth
    /// profile yet, as for a custom provider only the source defines.
    Absent,
    /// A state a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
