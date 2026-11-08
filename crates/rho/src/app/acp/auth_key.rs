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

use super::{
    auth_login::{show_until_signed_in, ShownLogin},
    AcpClientPort,
};
use crate::{
    app::credential_writes::{ApiKeyAuth, CredentialWriteError, KeyCheck},
    config::Config,
};

/// How long the key page waits for a key. Getting one may take a trip to
/// the provider's dashboard, like approving a device code, so this matches
/// Codex's device-code wait, the longest sign-in wait Rho has.
const PAGE_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// Signs `login` in for the `authenticate` request `request_id` through the
/// key page. [`crate::app::credential_writes`] checks each key against the
/// models endpoint `config` resolves and saves it to `store`, as it does for
/// the daemon's `rho/auth/key`; this serves the page.
pub(super) async fn serve_key_page(
    login: ApiKeyAuth,
    config: &Config,
    store: &dyn CredentialStore,
    request_id: RequestId,
    port: &dyn AcpClientPort,
) -> Result<(), AcpError> {
    let label = login.descriptor().display_name;
    let entry_label = login.entry_label();
    let page = KeyEntryPage::bind(KeyEntryLabels::new(label, entry_label))
        .await
        .map_err(|error| {
            AcpError::internal_error().data(format!("could not open the {label} key page: {error}"))
        })?;
    let url = page.url().to_owned();
    let entered = page
        .serve(PAGE_LIFETIME, move |key| {
            check_on_page(login, key, config, store)
        })
        .map(|served| {
            served.map_err(|error| {
                AcpError::internal_error().data(format!("could not sign in to {label}: {error}"))
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

/// Checks and saves a key entered on the page, telling the page what
/// happened: a refusal or a failed save lets the user try again, and a key
/// saved unchecked says so.
async fn check_on_page(
    login: ApiKeyAuth,
    key: EnteredKey,
    config: &Config,
    store: &dyn CredentialStore,
) -> KeyVerdict {
    let provider = login.descriptor().display_name;
    let saved = async {
        let check = login.check(key.expose(), config).await?;
        login.save(key.expose(), store)?;
        Ok::<_, CredentialWriteError>(check)
    };
    match saved.await {
        Ok(KeyCheck::Accepted) => KeyVerdict::Accepted { notice: None },
        Ok(KeyCheck::NoModelListing) => KeyVerdict::Accepted {
            notice: Some(format!(
                "Rho could not check the key with {provider}, so it saved the key unchecked."
            )),
        },
        Ok(KeyCheck::Unreachable(reason)) => KeyVerdict::Accepted {
            notice: Some(format!(
                "Rho could not check the key with {provider} ({reason}), so it saved the key unchecked."
            )),
        },
        Err(CredentialWriteError::KeyRefused { .. }) => {
            KeyVerdict::Rejected(format!("{provider} refused this key."))
        }
        // A failed save; the rest cannot follow a resolved profile.
        Err(
            error @ (CredentialWriteError::Store(_)
            | CredentialWriteError::UnknownAuth { .. }
            | CredentialWriteError::NotAnApiKey { .. }
            | CredentialWriteError::NoCredentials { .. }
            | CredentialWriteError::EnvironmentOverride { .. }
            | CredentialWriteError::BlankKey
            | CredentialWriteError::Config(_)),
        ) => KeyVerdict::Rejected(format!("Rho {error}.")),
    }
}

#[cfg(test)]
#[path = "auth_key_tests.rs"]
mod tests;
