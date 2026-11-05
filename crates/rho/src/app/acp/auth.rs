//! Sign-in uses the same login flows and credential store as the CLI.
//! Requests that fail for want of usable credentials answer `auth_required`,
//! so hosts offer these login methods instead of a generic failure.
//!
//! OAuth and API-key logins run in this process ([`AuthMethod::Agent`], see
//! [`super::auth_login`]) when [`sign_in`] says so; otherwise the host runs
//! `rho acp --login <auth>` in a terminal ([`AuthMethod::Terminal`]).

use agent_client_protocol::{
    schema::v1::{
        AuthMethod, AuthMethodAgent, AuthMethodTerminal, ClientCapabilities, InitializeRequest,
    },
    Error as AcpError, ErrorCode,
};
use rho_providers::{
    auth::browser::BrowserAvailability,
    model::ModelError,
    provider::{self, BearerCredentialAcquisition, BrowserOAuthFlow, ProviderAuthKind},
};

use super::host_features::{Forms, HostFeatures};

/// Whether a credential store has been chosen. A login must know where to
/// save, so until the choice is made `authenticate` asks it as a form, and
/// hosts without forms sign in through the terminal login, which asks too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CredentialStoreChoice {
    Made,
    Pending,
}

/// Where this connection's logins run, fixed at `initialize`. `initialize`
/// advertises from it and `authenticate` accepts from it, so the two cannot
/// disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SignIn {
    oauth: LoginSite,
    keys: LoginSite,
}

/// Where one kind of login runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoginSite {
    /// `authenticate` runs the login and shows its URL as an elicitation,
    /// after asking where to store credentials if no store is chosen.
    InProcess,
    /// The host's terminal runs `rho acp --login <auth>`, if it has one.
    Terminal,
}

impl SignIn {
    /// Every login in the host's terminal.
    #[cfg(test)]
    pub(super) const TERMINAL: Self = Self {
        oauth: LoginSite::Terminal,
        keys: LoginSite::Terminal,
    };

    /// Where a login that needs `needs` runs, if anywhere.
    fn site(self, needs: LoginNeeds) -> Option<LoginSite> {
        match needs {
            LoginNeeds::UrlThenWait => Some(self.oauth),
            LoginNeeds::KeyEntry => Some(self.keys),
            LoginNeeds::Unconfirmable => None,
        }
    }
}

/// `browser` says whether the user's browser runs where Rho does.
pub(super) fn sign_in(
    host: HostFeatures,
    store: CredentialStoreChoice,
    browser: BrowserAvailability,
) -> SignIn {
    let oauth = match (host.urls, store, host.forms) {
        (true, CredentialStoreChoice::Made, _)
        | (true, CredentialStoreChoice::Pending, Forms::Available(_)) => LoginSite::InProcess,
        (true, CredentialStoreChoice::Pending, Forms::Unavailable) | (false, _, _) => {
            LoginSite::Terminal
        }
    };
    // The key page listens on this machine's loopback. Over SSH or in a
    // container the host's browser cannot reach it, unlike a provider's
    // sign-in page, so keys stay with the terminal's hidden entry.
    let keys = match (oauth, browser) {
        (LoginSite::InProcess, BrowserAvailability::Graphical) => LoginSite::InProcess,
        (LoginSite::InProcess, BrowserAvailability::Headless) | (LoginSite::Terminal, _) => {
            LoginSite::Terminal
        }
    };
    SignIn { oauth, keys }
}

/// What finishing one auth mode's login takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoginNeeds {
    /// Typing a secret, which must never travel in an ACP message: hosts log
    /// them. The terminal's hidden entry or Rho's local key page takes it.
    KeyEntry,
    /// Opening a URL (with a device code, for device grants), then a wait for
    /// the provider to confirm.
    UrlThenWait,
    /// Nothing Rho can confirm finished; not advertised.
    Unconfirmable,
}

fn login_needs(kind: ProviderAuthKind) -> LoginNeeds {
    match kind {
        // Device-key setup only prints an authorization URL; there is no
        // completion to await, so it cannot promise sign-in success.
        ProviderAuthKind::None | ProviderAuthKind::OllamaDeviceKey { .. } => {
            LoginNeeds::Unconfirmable
        }
        ProviderAuthKind::ApiKey { .. } => LoginNeeds::KeyEntry,
        ProviderAuthKind::BearerCredential { acquisition, .. } => match acquisition {
            BearerCredentialAcquisition::BrowserOAuth(BrowserOAuthFlow::OpenRouter) => {
                LoginNeeds::UrlThenWait
            }
        },
        ProviderAuthKind::CodexOAuth { .. }
        | ProviderAuthKind::GithubCopilotDevice { .. }
        | ProviderAuthKind::XaiOAuth { .. }
        | ProviderAuthKind::KimiOAuth { .. }
        | ProviderAuthKind::MetaOAuth { .. } => LoginNeeds::UrlThenWait,
    }
}

/// A login `authenticate` runs in this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentLogin {
    /// OAuth or a device grant, shown as the provider's URL.
    OAuth,
    /// An API key, typed into Rho's local key page.
    ApiKey,
}

/// The login `authenticate` runs for `auth`, given `sign_in`, or `None` when
/// `auth` was not advertised as an agent method.
pub(super) fn agent_login(auth: &str, sign_in: SignIn) -> Option<AgentLogin> {
    let (_, mode) = provider::resolve_auth_mode(auth)?;
    let needs = login_needs(mode.auth_kind);
    match sign_in.site(needs)? {
        LoginSite::InProcess => {}
        LoginSite::Terminal => return None,
    }
    match needs {
        LoginNeeds::UrlThenWait => Some(AgentLogin::OAuth),
        LoginNeeds::KeyEntry => Some(AgentLogin::ApiKey),
        LoginNeeds::Unconfirmable => None,
    }
}

pub(super) fn methods(request: &InitializeRequest, sign_in: SignIn) -> Vec<AuthMethod> {
    let capabilities: &ClientCapabilities = &request.client_capabilities;
    let terminal = capabilities.auth.terminal;
    provider::providers()
        .into_iter()
        .flat_map(|provider| provider.auth_modes())
        .filter_map(|mode| match sign_in.site(login_needs(mode.auth_kind))? {
            LoginSite::InProcess => Some(AuthMethod::Agent(AuthMethodAgent::new(
                mode.id,
                mode.login_label,
            ))),
            LoginSite::Terminal if terminal => Some(AuthMethod::Terminal(
                AuthMethodTerminal::new(mode.id, mode.login_label)
                    .args(vec!["--login".into(), mode.id.into()]),
            )),
            LoginSite::Terminal => None,
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
