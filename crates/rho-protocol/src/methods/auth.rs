//! Signing the daemon in to a provider with an API key, and out (#1662).
//!
//! The daemon writes the credential store its sessions read, so a key saved
//! or a logout made by one client applies to every session the daemon runs,
//! from each session's next prompt.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Check an API key with its provider and save it.
pub const AUTH_KEY: &str = "rho/auth/key";
/// Delete an auth profile's saved credentials.
pub const AUTH_LOGOUT: &str = "rho/auth/logout";

/// Params of [`AUTH_KEY`].
///
/// The key travels only over the daemon's socket, which is private to the
/// daemon's user (mode 0600) and refuses peers of another user, and the
/// daemon never logs it. `Debug` shows `[redacted]` in its place, so a
/// client that logs its requests does not log the key either.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthKeyParams {
    /// The auth profile the key signs in to, such as `openai-api-key` or a
    /// custom provider's `<name>-api-key`. It must take an API key.
    pub auth: String,
    /// The API key. Surrounding whitespace is ignored.
    pub key: String,
}

impl fmt::Debug for AuthKeyParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthKeyParams")
            .field("auth", &self.auth)
            .field("key", &"[redacted]")
            .finish()
    }
}

/// Result of [`AUTH_KEY`]: the key is saved to the credential store the
/// daemon's config selects. Running sessions on the auth profile use it from
/// their next prompt; new sessions use it at once.
///
/// The daemon first asks the provider's models endpoint about the key and
/// refuses one the provider refuses with
/// [`crate::ErrorCode::KeyRefused`], saving nothing. A key the provider
/// cannot be asked about, because rho lists no models for it or the endpoint
/// did not answer, is saved unchecked, as `rho login` saves it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthKeyResponse {
    /// Why the key was saved without the provider checking it. Absent when
    /// the provider accepted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unchecked: Option<String>,
}

/// Params of [`AUTH_LOGOUT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthLogoutParams {
    /// The auth profile to sign out of. Its provider's endpoint login, as
    /// Ollama records one, is forgotten too.
    pub auth: String,
}

/// Result of [`AUTH_LOGOUT`]: the auth profile's saved credentials are
/// deleted, if it had any. Running sessions on the profile drop them from
/// their next prompt, which then fails with
/// [`crate::TurnErrorKind::Authentication`] until a key is saved again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthLogoutResponse {}

/// The `data` of a [`crate::ErrorCode::EnvironmentOverride`] error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentOverride {
    /// The variable in the daemon's environment that holds the auth
    /// profile's credential, such as `OPENAI_API_KEY`.
    pub variable: String,
}
