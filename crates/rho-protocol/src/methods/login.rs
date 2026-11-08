//! Signing the daemon in with a provider's interactive login, relayed to the
//! client that asked (#1662).
//!
//! The login runs in the daemon, which saves the credential its sessions
//! read; the client only shows what the user must do. [`AUTH_LOGIN`]
//! answers at once with the login's id, then the daemon sends that client
//! [`AUTH_LOGIN_PROMPT`] with the URL to open (and any device code), and
//! [`AUTH_LOGIN_FINISHED`] once the login ended, on the connection that
//! started it. A login belongs to that connection: when it closes, the
//! daemon cancels the login.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Start an auth profile's interactive login.
pub const AUTH_LOGIN: &str = "rho/auth/login";
/// Cancel a login under way.
pub const AUTH_LOGIN_CANCEL: &str = "rho/auth/login_cancel";
/// What the user must do to finish a login.
pub const AUTH_LOGIN_PROMPT: &str = "rho/auth/login_prompt";
/// A login ended.
pub const AUTH_LOGIN_FINISHED: &str = "rho/auth/login_finished";

/// One login the daemon runs, unique across the daemon's life and
/// unguessable, so only the client that started it can name it.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct LoginId(String);

impl LoginId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LoginId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Params of [`AUTH_LOGIN`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthLoginParams {
    /// The auth profile to sign in to, such as `openai-codex`. It must take
    /// an interactive login ([`crate::AuthKind::Login`]); an API key goes
    /// through [`crate::methods::AUTH_KEY`].
    pub auth: String,
    /// How the user finishes a provider that offers both flows, `browser`
    /// when absent. A provider with one flow ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<LoginFlow>,
}

/// How the user finishes a login whose provider offers both flows. The
/// daemon reads it, so it has no `Unknown`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginFlow {
    /// The provider's page redirects the browser to a callback the daemon
    /// listens for on its machine's loopback, so the browser must run on
    /// that machine.
    #[default]
    Browser,
    /// The user enters a code on any device, as over SSH.
    DeviceCode,
}

/// Result of [`AUTH_LOGIN`]: the login started. Its prompt and its end
/// follow as notifications on this connection, never before this response.
/// A second login to an auth profile with one under way, from any client,
/// is refused with [`crate::ErrorCode::LoginBusy`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthLoginResponse {
    pub login: LoginId,
}

/// Params of [`AUTH_LOGIN_CANCEL`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthLoginCancelParams {
    pub login: LoginId,
}

/// Result of [`AUTH_LOGIN_CANCEL`]: the daemon is stopping the login, which
/// [`AUTH_LOGIN_FINISHED`] then reports `cancelled`. A login the provider
/// already approved still saves and ends `succeeded`. A login that already
/// ended, or no login, has the id: refused with
/// [`crate::ErrorCode::LoginNotFound`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthLoginCancelResponse {}

/// Params of the [`AUTH_LOGIN_PROMPT`] notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthLoginPromptParams {
    pub login: LoginId,
    pub prompt: LoginPrompt,
}

/// What the user must do to finish a login. The daemon opens no browser:
/// the client opens [`Self::url_with_code`], else [`Self::url`], where it
/// can, and always shows the URL too.
///
/// `Debug` hides the device code, which signs in whoever enters it first.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoginPrompt {
    /// The page to open: the provider's sign-in page, or for a device-code
    /// flow its verification page.
    pub url: String,
    /// The code to enter on that page, for a device-code flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    /// The verification page with the code filled in, when the provider
    /// gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_with_code: Option<String>,
    /// What to do, in words, for a person.
    pub instruction: String,
}

impl fmt::Debug for LoginPrompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = |value: &Option<String>| value.as_ref().map(|_| "[redacted]");
        formatter
            .debug_struct("LoginPrompt")
            .field("url", &self.url)
            .field("user_code", &redacted(&self.user_code))
            .field("url_with_code", &redacted(&self.url_with_code))
            .field("instruction", &self.instruction)
            .finish()
    }
}

/// Params of the [`AUTH_LOGIN_FINISHED`] notification, the last one a login
/// sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthLoginFinishedParams {
    pub login: LoginId,
    pub outcome: LoginOutcome,
}

/// How a login ended. A login that saved a credential applies to every
/// session the daemon runs, as [`crate::methods::AUTH_KEY`] does: running
/// sessions use it from their next prompt, and each attached client gets
/// the sessions' new options.
///
/// A provider that sends no confirmation (Ollama Cloud's device key)
/// succeeds as soon as the daemon saved the login: its prompt's
/// `instruction` already told the user what to approve.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LoginOutcome {
    /// The provider approved the login, and the daemon saved the
    /// credential.
    Succeeded,
    /// The login failed; `message` says why, for a person.
    Failed { message: String },
    /// A client cancelled it ([`AUTH_LOGIN_CANCEL`]).
    Cancelled,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
