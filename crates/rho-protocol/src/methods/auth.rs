//! The auth profiles the daemon can sign in to, and signing it in to a
//! provider with an API key, and out (#1662). Interactive logins are in
//! `login.rs`.
//!
//! The daemon writes the credential store its sessions read, so a key saved
//! or a logout made by one client applies to every session the daemon runs,
//! from each session's next prompt.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ClientEnvironment;

/// List the auth profiles a client can sign the daemon in to. The ACP
/// adapter's `initialize` reads it for its auth methods (#1670).
pub const AUTH_LIST: &str = "rho/auth/list";
/// Check an API key with its provider and save it.
pub const AUTH_KEY: &str = "rho/auth/key";
/// Delete an auth profile's saved credentials.
pub const AUTH_LOGOUT: &str = "rho/auth/logout";

/// Params of [`AUTH_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthListParams {
    /// The asking client's environment, whose credential variables and
    /// `RHO_CREDENTIAL_STORE` are that client's sessions' (#1779): a profile one of its
    /// variables holds a credential for is listed as signed in.
    /// An empty one names no store and holds no credential, so the daemon's
    /// own store is used.
    pub environment: ClientEnvironment,
}

/// Result of [`AUTH_LIST`]: every auth profile that takes a credential, as
/// the TUI's `/login` picker and ACP's auth methods offer them, with the
/// custom providers of the daemon's config. Keyless profiles are left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthListResponse {
    pub auths: Vec<AuthSummary>,
}

/// One auth profile a client can sign in to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthSummary {
    /// The profile's id, which the auth methods take, such as
    /// `openai-codex`.
    pub auth: String,
    /// The provider it signs in to, such as `openai`.
    pub provider: String,
    /// What a picker shows for it, such as `ChatGPT Plus/Pro (Codex)`.
    pub label: String,
    pub kind: AuthKind,
    pub status: AuthStatus,
}

/// How a client signs in to an auth profile. Protocol minors may add kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuthKind {
    /// With an API key, through [`AUTH_KEY`].
    ApiKey,
    /// With the provider's interactive login, which the daemon runs and
    /// relays to the client, through [`crate::methods::AUTH_LOGIN`].
    Login,
    /// A kind a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Whether the daemon's sessions can use an auth profile now. Protocol
/// minors may add values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuthStatus {
    /// It has a credential: saved, or in the daemon's environment.
    SignedIn,
    SignedOut,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

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
    /// The asking client's environment, whose credential variables and
    /// `RHO_CREDENTIAL_STORE` are that client's sessions' (#1779): the key is saved to the
    /// store it chooses, and refused as
    /// [`crate::ErrorCode::EnvironmentOverride`] when one of its variables
    /// already holds the profile's credential.
    /// An empty one names no store and holds no credential, so the daemon's
    /// own store is used.
    pub environment: ClientEnvironment,
}

impl fmt::Debug for AuthKeyParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthKeyParams")
            .field("auth", &self.auth)
            .field("key", &"[redacted]")
            .field("environment", &self.environment)
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
/// cannot be asked about, because rho lists no models for it, the endpoint
/// did not answer, or the key may not list models, is saved unchecked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthKeyResponse {
    /// Why the key was saved without the provider checking it. Absent when
    /// the provider accepted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unchecked: Option<UncheckedKey>,
}

/// Why [`AUTH_KEY`] saved a key the provider did not check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UncheckedKey {
    /// Why, for a program to branch on.
    pub reason: UncheckedReason,
    /// The reason in words, for a person: what went wrong when the provider
    /// could not be reached, such as `HTTP 404`. It never contains the key.
    pub message: String,
}

/// Why a key went unchecked. Protocol minors may add reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UncheckedReason {
    /// Rho lists no models for the provider, so it cannot ask about a key.
    NoModelListing,
    /// The provider's models endpoint did not answer the question: it could
    /// not be reached, or it failed with a status other than a refusal, as a
    /// gateway without `/models` does.
    Unreachable,
    /// The provider's models endpoint answered 403 Forbidden: the key may
    /// not list models, as a key restricted to running them may not, so
    /// the endpoint says nothing about whether it works.
    Forbidden,
    /// A reason a newer daemon sends that this crate does not know. Reading
    /// it leniently keeps the response usable; daemons never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Params of [`AUTH_LOGOUT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthLogoutParams {
    /// The auth profile to sign out of. Its provider's endpoint login, as
    /// Ollama records one, is forgotten too.
    pub auth: String,
    /// The asking client's environment, whose credential variables and
    /// `RHO_CREDENTIAL_STORE` are that client's sessions' (#1779): the credential is deleted
    /// from the store it chooses, and the request refused as
    /// [`crate::ErrorCode::EnvironmentOverride`] when one of its variables
    /// holds the profile's credential.
    /// An empty one names no store and holds no credential, so the daemon's
    /// own store is used.
    pub environment: ClientEnvironment,
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
    /// The variable in the asking client's environment that holds the auth
    /// profile's credential, such as `OPENAI_API_KEY`, or the
    /// `RHO_CREDENTIAL_STORE` that chooses the credential store, in the
    /// client's environment or the daemon's own.
    pub variable: String,
}
