//! `authenticate` for API keys, on hosts that open URL elicitations.
//!
//! Hosts log ACP traffic, so a key must never travel in an ACP message.
//! Instead Rho serves a one-time key page on `127.0.0.1` (see
//! [`rho_providers::auth::key_entry`]) and sends its URL as a URL-mode
//! `elicitation/create` scoped to the `authenticate` request. The browser
//! posts the key straight back to Rho, which checks it against the
//! provider's models endpoint and saves it only if the provider accepts it.
//! A rejected key is reported on the page, where the user can try again.

use std::time::Duration;

use agent_client_protocol::{schema::v1::RequestId, Error as AcpError};
use futures_util::FutureExt;
use rho_providers::{
    auth::key_entry::{EnteredKey, KeyEntryLabels, KeyEntryPage, KeyVerdict},
    credentials::CredentialStore,
};
use url::Url;

use super::{
    auth_login::{show_until_signed_in, ShownLogin},
    AcpClientPort,
};
use crate::{
    app::{
        config_repository::ConfigRepository,
        sign_in::{ApiKeyAuth, KeyCheck, KeyError},
    },
    config::Config,
};

/// How long the key page waits for a key. Getting one may take a trip to
/// the provider's dashboard, like approving a device code, so this matches
/// Codex's device-code wait, the longest sign-in wait Rho has.
const PAGE_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// An API-key login that can finish: `auth` takes a key, and no environment
/// variable overrides a saved one. [`crate::app::sign_in`] checks and saves
/// each key, as it does for the daemon's `rho/auth/key`; this serves the page.
#[derive(Clone, Copy)]
pub(super) struct KeyLogin<'a>(ApiKeyAuth<'a>);

impl<'a> KeyLogin<'a> {
    /// Checks `auth` before anything is asked of the user, so a login that
    /// cannot finish does not first ask where to store credentials.
    pub(super) fn prepare(auth: &'a str) -> Result<Self, AcpError> {
        ApiKeyAuth::resolve(auth)
            .map(Self)
            .map_err(|error| match error {
                KeyError::UnknownAuth { .. } | KeyError::NotAnApiKeyAuth { .. } => {
                    AcpError::invalid_params().data(error.to_string())
                }
                KeyError::EnvironmentOverride { .. } => {
                    AcpError::invalid_request().data(error.to_string())
                }
                // Resolving checks the profile only.
                KeyError::Blank
                | KeyError::Rejected { .. }
                | KeyError::Open(_)
                | KeyError::Save { .. }
                | KeyError::RecordLogin(_) => AcpError::internal_error().data(error.to_string()),
            })
    }

    /// Where the key is checked; see [`ApiKeyAuth::endpoint`].
    pub(super) fn endpoint(&self, config: &Config) -> Option<Url> {
        self.0.endpoint(config)
    }

    /// Records the login once the key is saved; see
    /// [`ApiKeyAuth::record_login`].
    pub(super) fn record_login(&self, repository: &ConfigRepository) -> Result<(), AcpError> {
        self.0
            .record_login(repository)
            .map_err(|error| AcpError::internal_error().data(error.to_string()))
    }

    /// Signs in for the `authenticate` request `request_id`, saving the key
    /// to `store`. `endpoint` is [`ApiKeyAuth::endpoint`].
    pub(super) async fn sign_in(
        self,
        endpoint: Option<&Url>,
        store: &dyn CredentialStore,
        request_id: RequestId,
        port: &dyn AcpClientPort,
    ) -> Result<(), AcpError> {
        let Self(login) = self;
        let label = login.descriptor().display_name;
        let entry_label = login.entry_label();
        let page = KeyEntryPage::bind(KeyEntryLabels::new(label, entry_label))
            .await
            .map_err(|error| {
                AcpError::internal_error()
                    .data(format!("could not open the {label} key page: {error}"))
            })?;
        let url = page.url().to_owned();
        let entered = page
            .serve(PAGE_LIFETIME, move |key| {
                check_on_page(login, key, endpoint, store)
            })
            .map(|served| {
                served.map_err(|error| {
                    AcpError::internal_error()
                        .data(format!("could not sign in to {label}: {error}"))
                })
            });
        let shown = ShownLogin {
            auth: login.auth(),
            provider_label: label,
            url: &url,
            message: format!("Enter your {entry_label} on the page that opens."),
        };
        show_until_signed_in(port, request_id, shown, entered).await
    }
}

/// Checks and saves a key entered on the page, telling the page what
/// happened: a refusal or a failed save lets the user try again, and a key
/// saved unchecked says so.
async fn check_on_page(
    login: ApiKeyAuth<'_>,
    key: EnteredKey,
    endpoint: Option<&Url>,
    store: &dyn CredentialStore,
) -> KeyVerdict {
    let provider = login.descriptor().display_name;
    match login.check_and_save(key.expose(), endpoint, store).await {
        Ok(KeyCheck::Accepted) => KeyVerdict::Accepted { notice: None },
        Ok(KeyCheck::Unsupported) => KeyVerdict::Accepted {
            notice: Some(format!(
                "Rho could not check the key with {provider}, so it saved the key unchecked."
            )),
        },
        Ok(KeyCheck::Unreachable(reason)) => KeyVerdict::Accepted {
            notice: Some(format!(
                "Rho could not check the key with {provider} ({reason}), so it saved the key unchecked."
            )),
        },
        Err(KeyError::Rejected { .. }) => {
            KeyVerdict::Rejected(format!("{provider} refused this key."))
        }
        Err(KeyError::Save { detail }) => {
            KeyVerdict::Rejected(format!("Could not save the key: {detail}"))
        }
        // The page posts a trimmed key that is never blank, and the profile
        // was resolved before the page opened.
        Err(
            error @ (KeyError::Blank
            | KeyError::UnknownAuth { .. }
            | KeyError::NotAnApiKeyAuth { .. }
            | KeyError::EnvironmentOverride { .. }
            | KeyError::Open(_)
            | KeyError::RecordLogin(_)),
        ) => KeyVerdict::Rejected(error.to_string()),
    }
}

#[cfg(test)]
#[path = "auth_key_tests.rs"]
mod tests;
