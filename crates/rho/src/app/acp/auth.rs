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

/// Shown when the provider refused credentials it was given. Provider
/// messages and diagnostics may quote the request, so none reach the host.
const REJECTED_MESSAGE: &str = "the provider rejected the saved credentials; sign in again";

/// A failure that signing in can fix: the selected auth profile has no
/// credentials, or the provider refused the ones it had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CredentialFailure {
    /// Nothing is saved or set in the environment for the selected auth. The
    /// message comes from the provider descriptor table, never from a
    /// credential, and names the provider and how to sign in.
    Missing(&'static str),
    /// The provider answered 401/403 or reported an authentication failure:
    /// an expired, revoked, or wrong token or key.
    Rejected,
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
                ModelError::HttpStatus { status, .. } if matches!(status.as_u16(), 401 | 403) => {
                    Some(Self::Rejected)
                }
                // A credential store that cannot be read is broken, not
                // signed out; a fresh login would hit the same store.
                ModelError::Credentials(_)
                | ModelError::Request(_)
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
        // `rho_sdk::Error` is non-exhaustive; other variants are not
        // credential failures.
        match cause.downcast_ref::<rho_sdk::Error>()? {
            rho_sdk::Error::Authentication { .. } => Some(Self::Rejected),
            rho_sdk::Error::Provider(error)
                if error.kind() == rho_sdk::ProviderErrorKind::Authentication =>
            {
                Some(Self::Rejected)
            }
            _ => None,
        }
    }

    /// ACP `auth_required` with a Rho-owned message and no data, so neither
    /// a credential nor a provider auth diagnostic reaches the host.
    pub(super) fn into_acp(self) -> AcpError {
        let message = match self {
            Self::Missing(message) => message,
            Self::Rejected => REJECTED_MESSAGE,
        };
        AcpError::new(i32::from(ErrorCode::AuthRequired), message)
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
