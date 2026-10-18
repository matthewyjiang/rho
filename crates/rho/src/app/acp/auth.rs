//! Terminal authentication uses the same login and credential store as the CLI.
//! Requests that fail for want of usable credentials answer `auth_required`,
//! so hosts offer these login methods instead of a generic failure.

use agent_client_protocol::{
    schema::v1::{AuthMethod, AuthMethodTerminal, ClientCapabilities},
    Error as AcpError, ErrorCode,
};
use rho_providers::{
    model::ModelError,
    provider::{self, ProviderAuthKind},
};

pub(super) fn methods(capabilities: &ClientCapabilities) -> Vec<AuthMethod> {
    if !capabilities.auth.terminal {
        return Vec::new();
    }
    provider::providers()
        .into_iter()
        .flat_map(|provider| provider.auth_modes())
        .filter(|mode| match mode.auth_kind {
            // Device-key setup only prints an authorization URL; there is no
            // completion to await, so it cannot promise terminal-auth success.
            ProviderAuthKind::None | ProviderAuthKind::OllamaDeviceKey { .. } => false,
            ProviderAuthKind::ApiKey { .. }
            | ProviderAuthKind::CodexOAuth { .. }
            | ProviderAuthKind::GithubCopilotDevice { .. }
            | ProviderAuthKind::XaiOAuth { .. }
            | ProviderAuthKind::BearerCredential { .. }
            | ProviderAuthKind::KimiOAuth { .. }
            | ProviderAuthKind::MetaOAuth { .. } => true,
        })
        .map(|mode| {
            AuthMethod::Terminal(
                AuthMethodTerminal::new(mode.id, mode.login_label)
                    .args(vec!["--login".into(), mode.id.into()]),
            )
        })
        .collect()
}

/// A failure that signing in can fix. This follows the provider layer, which
/// files each of these under `ProviderErrorKind::Authentication` when it
/// converts them for the SDK, so startup and prompt failures agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CredentialFailure {
    /// Nothing is saved or set in the environment for the selected auth. The
    /// message comes from the provider descriptor table, never from a
    /// credential, and names the provider and how to sign in.
    Missing(&'static str),
    /// The credential store failed to read or refresh the saved credentials;
    /// signing in again saves a fresh copy.
    Unreadable,
    /// The provider answered 401/403 or reported an authentication failure:
    /// an expired, revoked, or wrong token or key.
    Refused,
}

impl CredentialFailure {
    /// Finds a credential failure in `error`'s cause chain. Only typed causes
    /// count, so rewording an error message cannot reclassify it.
    pub(super) fn find(error: &anyhow::Error) -> Option<Self> {
        error.chain().find_map(Self::of_cause)
    }

    fn of_cause(cause: &(dyn std::error::Error + 'static)) -> Option<Self> {
        if let Some(error) = cause.downcast_ref::<ModelError>() {
            return match error {
                ModelError::MissingCredentials(message) => Some(Self::Missing(message)),
                ModelError::Credentials(_) => Some(Self::Unreadable),
                ModelError::HttpStatus { status, .. } if matches!(status.as_u16(), 401 | 403) => {
                    Some(Self::Refused)
                }
                ModelError::Request(_)
                | ModelError::HttpStatus { .. }
                | ModelError::Io(_)
                | ModelError::Interrupted
                | ModelError::StreamIdleTimeout { .. }
                | ModelError::StreamFailedAfterOutput { .. }
                | ModelError::InvalidResponse(_)
                | ModelError::RetryableInvalidResponse { .. }
                | ModelError::ProviderReported { .. }
                | ModelError::UnsupportedReasoning { .. }
                | ModelError::UnsupportedProvider(_) => None,
            };
        }
        // The SDK error no longer says which credential failure it was.
        // `rho_sdk::Error` is non-exhaustive; other variants are not
        // credential failures.
        match cause.downcast_ref::<rho_sdk::Error>()? {
            rho_sdk::Error::Authentication { .. } => Some(Self::Refused),
            rho_sdk::Error::Provider(error)
                if error.kind() == rho_sdk::ProviderErrorKind::Authentication =>
            {
                Some(Self::Refused)
            }
            _ => None,
        }
    }

    /// ACP `auth_required` with a Rho-owned message and no data. Provider
    /// messages and diagnostics may quote the request, so none reach the host.
    fn into_acp(self) -> AcpError {
        let message = match self {
            Self::Missing(message) => message,
            Self::Unreadable => "could not read the stored credentials; sign in again",
            // After SDK conversion a 401, a 403 (which can mean the account
            // lacks access), and a store failure look alike.
            Self::Refused => {
                "could not authenticate with the provider; sign in again or check the account's access"
            }
        };
        AcpError::new(i32::from(ErrorCode::AuthRequired), message)
    }
}

/// Maps a failed session request for the host: `auth_required` when signing
/// in can fix it, otherwise `internal_error` carrying `detail`.
pub(super) fn request_error(error: &anyhow::Error, detail: impl FnOnce() -> String) -> AcpError {
    match CredentialFailure::find(error) {
        Some(failure) => failure.into_acp(),
        None => AcpError::internal_error().data(detail()),
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
