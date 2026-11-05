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
    auth::{
        key_entry::{EnteredKey, KeyEntryLabels, KeyEntryPage, KeyVerdict},
        login_dispatch::ProviderAuthentication,
    },
    credentials::CredentialStore,
    model::{
        provider_models::{check_api_key, ProviderModelEndpoint},
        ModelError,
    },
    provider::{self, ProviderAuthKind, ProviderDescriptor},
};
use url::Url;

use super::{
    auth_login::{show_until_signed_in, ShownLogin},
    AcpClientPort,
};

/// How long the key page waits for a key. Getting one may take a trip to
/// the provider's dashboard, like approving a device code, so this matches
/// Codex's device-code wait, the longest sign-in wait Rho has.
const PAGE_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// An API-key login that can finish: `auth` takes a key, and no environment
/// variable overrides a saved one.
pub(super) struct KeyLogin<'a> {
    auth: &'a str,
    descriptor: &'static ProviderDescriptor,
    entry_label: &'static str,
}

impl<'a> KeyLogin<'a> {
    /// Checks `auth` before anything is asked of the user, so a login that
    /// cannot finish does not first ask where to store credentials.
    pub(super) fn prepare(auth: &'a str) -> Result<Self, AcpError> {
        let Some((descriptor, mode)) = provider::resolve_auth_mode(auth) else {
            return Err(AcpError::invalid_params().data(format!("unknown auth profile '{auth}'")));
        };
        let ProviderAuthKind::ApiKey {
            env_var,
            entry_label,
            ..
        } = mode.auth_kind
        else {
            return Err(
                AcpError::invalid_params().data(format!("'{auth}' does not take an API key"))
            );
        };
        // The variable wins over a saved key, so saving one would change
        // nothing and the check would test the variable's key instead.
        if ProviderAuthentication::has_environment_override(auth) {
            return Err(AcpError::invalid_request().data(format!(
                "'{auth}' reads its key from {env_var}; unset it to sign in with a saved key"
            )));
        }
        Ok(Self {
            auth,
            descriptor,
            entry_label,
        })
    }

    /// Signs in for the `authenticate` request `request_id`, saving the key
    /// to `store`. `endpoint` is the API base config resolves for an
    /// OpenAI-compatible provider, where its models endpoint lives.
    pub(super) async fn sign_in(
        self,
        endpoint: Option<&Url>,
        store: &dyn CredentialStore,
        request_id: RequestId,
        port: &dyn AcpClientPort,
    ) -> Result<(), AcpError> {
        let Self {
            auth,
            descriptor,
            entry_label,
        } = self;
        let label = descriptor.display_name;
        let page = KeyEntryPage::bind(KeyEntryLabels::new(label, entry_label))
            .await
            .map_err(|error| {
                AcpError::internal_error()
                    .data(format!("could not open the {label} key page: {error}"))
            })?;
        let url = page.url().to_owned();
        let endpoint = endpoint.map_or(
            ProviderModelEndpoint::ProviderOwned,
            ProviderModelEndpoint::OpenAiCompatible,
        );
        let entered = page
            .serve(PAGE_LIFETIME, move |key| {
                check_and_save(auth, descriptor, key, endpoint, store)
            })
            .map(|served| {
                served.map_err(|error| {
                    AcpError::internal_error()
                        .data(format!("could not sign in to {label}: {error}"))
                })
            });
        let shown = ShownLogin {
            auth,
            provider_label: label,
            url: &url,
            message: format!("Enter your {entry_label} on the page that opens."),
        };
        show_until_signed_in(port, request_id, shown, entered).await
    }
}

/// Saves `key` unless the provider refuses it. A key the provider cannot be
/// asked about (no model listing in Rho, an outage, a gateway without `/models`)
/// is saved as the terminal login saves it, and the page says it went
/// unchecked, so such a provider can still be signed in to.
async fn check_and_save(
    auth: &str,
    descriptor: &ProviderDescriptor,
    key: EnteredKey,
    endpoint: ProviderModelEndpoint<'_>,
    store: &dyn CredentialStore,
) -> KeyVerdict {
    let key = key.expose();
    let provider = descriptor.display_name;
    let notice = if descriptor.supports_model_refresh() {
        match check_api_key(auth, key, endpoint).await {
            Ok(()) => None,
            Err(error) => match unchecked_reason(&error) {
                None => return KeyVerdict::Rejected(format!("{provider} refused this key.")),
                Some(reason) => Some(redacted(
                    format!(
                        "Rho could not check the key with {provider} ({reason}), so it saved the key unchecked."
                    ),
                    key,
                )),
            },
        }
    } else {
        Some(format!(
            "Rho could not check the key with {provider}, so it saved the key unchecked."
        ))
    };
    match ProviderAuthentication::save_api_key(store, auth, key) {
        Ok(()) => KeyVerdict::Accepted { notice },
        Err(error) => {
            KeyVerdict::Rejected(redacted(format!("Could not save the key: {error}"), key))
        }
    }
}

/// Why checking a key failed without the provider refusing it, or `None`
/// when it refused. A plain `GET /models` that carries only the key fails
/// 400, 401, or 403 only for the key itself; Google answers a malformed key
/// with 400. Response bodies may quote the request, so only statuses show.
fn unchecked_reason(error: &ModelError) -> Option<String> {
    let status = match error {
        ModelError::MissingCredentials(_) => return None,
        ModelError::HttpStatus { status, .. } => Some(*status),
        ModelError::Request(error) => match error.status() {
            Some(status) => Some(status),
            None if error.is_connect() || error.is_timeout() => {
                return Some("could not reach it".into());
            }
            None => return Some(error.to_string()),
        },
        ModelError::Credentials(_)
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
    match status {
        Some(status) if matches!(status.as_u16(), 400 | 401 | 403) => None,
        Some(status) => Some(format!("HTTP {status}")),
        None => Some(error.to_string()),
    }
}

/// `message` with any copy of `key` removed, in case an error quoted it.
fn redacted(message: String, key: &str) -> String {
    message.replace(key, "[key]")
}

#[cfg(test)]
#[path = "auth_key_tests.rs"]
mod tests;
