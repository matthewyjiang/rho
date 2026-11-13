//! `rho/auth/list`, `rho/auth/key`, and `rho/auth/logout`: the auth
//! profiles a client can sign in to, and the daemon writing the credential
//! store its sessions read (#1662). Interactive logins are `logins.rs`'s.
//!
//! [`crate::app::credential_writes`] does the work, checking and saving
//! keys and signing out against the store the daemon's config selects
//! when the request runs. Every write
//! moves the process's credential generation, so each running session uses
//! a new key, or drops a deleted one, from its next prompt, and
//! [`credentials_changed`] refreshes the options of each session not in a
//! turn, whose models to switch to follow the store. Config and the OS
//! keyring block, so those steps run off the async workers.
//!
//! A key reaches the daemon only over its private socket. Nothing here logs
//! a request's params, and errors that could quote a key have it removed
//! before they leave [`crate::app::credential_writes`].

use std::sync::Arc;

use rho_protocol::{
    AuthKeyParams, AuthKeyResponse, AuthKind, AuthListParams, AuthListResponse, AuthLogoutParams,
    AuthLogoutResponse, AuthStatus, AuthSummary, ClientEnvironment, ErrorCode, RpcError,
    UncheckedKey, UncheckedReason,
};
use rho_providers::{
    auth::login_dispatch::{AuthenticationMethod, ProviderAuthentication},
    credentials::{available_auth_modes, CredentialStore},
    model::catalog::{self, LoginTarget},
};

use super::sessions::{session_environment, Sessions};
use crate::{
    app::{
        config_repository::ConfigRepository,
        credential_writes::{self, ApiKeyAuth, CredentialWriteError, KeyCheck},
    },
    credential_store::AppCredentialStore,
};

/// The credentials of the client that sent `environment` with a sign-in
/// request: the ones its sessions read (see
/// [`crate::credential_store::ClientCredentialStore`]), so what it saves or
/// deletes is what they then find.
pub(super) fn client_credentials(
    environment: &ClientEnvironment,
) -> Result<Arc<dyn CredentialStore>, RpcError> {
    Ok(session_environment(environment)?.credentials())
}

/// The config the daemon reads, found as a session open finds it.
pub(super) fn repository() -> ConfigRepository {
    ConfigRepository::new(None)
}

/// The auth profiles a client can sign in to: the login targets the TUI's
/// `/login` picker offers, which are ACP's auth methods too, with the
/// custom providers of the daemon's config.
pub(super) async fn list(params: AuthListParams) -> Result<AuthListResponse, RpcError> {
    let credentials = client_credentials(&params.environment)?;
    let auths = off_workers(move || -> Result<_, CredentialWriteError> {
        credential_writes::load_config_and_store(&repository())?;
        // The TUI and ACP still sign in in their own processes until they
        // become daemon clients (#1667, #1670), so their logins are new to
        // the OS store's cache.
        AppCredentialStore::reread_external_logins();
        let signed_in = available_auth_modes(credentials.as_ref());
        Ok(catalog::login_targets()
            .into_iter()
            .filter_map(|target| summary(target, &signed_in))
            .collect::<Vec<_>>())
    })
    .await??;
    Ok(AuthListResponse { auths })
}

/// `target` as `rho/auth/list` reports it, given the auths `signed_in`;
/// `None` for one that takes no credential.
fn summary(target: LoginTarget, signed_in: &[String]) -> Option<AuthSummary> {
    let kind = match ProviderAuthentication::method(&target.auth).ok()? {
        AuthenticationMethod::None => return None,
        AuthenticationMethod::ApiKey { .. } => AuthKind::ApiKey,
        AuthenticationMethod::Interactive { .. } => AuthKind::Login,
    };
    let status = if signed_in.contains(&target.auth) {
        AuthStatus::SignedIn
    } else {
        AuthStatus::SignedOut
    };
    let LoginTarget {
        provider,
        auth,
        label,
    } = target;
    Some(AuthSummary {
        auth,
        provider,
        label,
        kind,
        status,
    })
}

/// What every credential write is followed by: forget the OS store's
/// cached reads, so the refresh sees this write and any login another
/// process saved since (the TUI's or ACP's, until #1667 and #1670), then
/// refresh every loaded session's options, whose models follow the store.
pub(super) async fn credentials_changed(sessions: &Sessions) {
    AppCredentialStore::reread_external_logins();
    sessions.refresh_all_options().await;
}

/// Checks `params.key` with its provider and saves it. Only the provider's
/// check runs on the async workers.
pub(super) async fn save_key(params: AuthKeyParams) -> Result<AuthKeyResponse, RpcError> {
    let AuthKeyParams {
        auth,
        key,
        environment,
    } = params;
    let credentials = client_credentials(&environment)?;
    let asked = Arc::clone(&credentials);
    let (config, login, key) = off_workers(move || -> Result<_, CredentialWriteError> {
        let config = credential_writes::load_config_and_store(&repository())?;
        let login = ApiKeyAuth::resolve(&auth, asked.as_ref())?;
        let key = key.trim();
        if key.is_empty() {
            return Err(CredentialWriteError::BlankKey);
        }
        Ok((config, login, key.to_owned()))
    })
    .await??;
    let check = login.check(&key, &config).await?;
    off_workers(move || {
        login.save(&key, credentials.as_ref())?;
        login.record_login(&repository())
    })
    .await??;
    tracing::info!(auth = %login.auth(), "saved an API key");
    let unchecked = match check {
        KeyCheck::Accepted => None,
        KeyCheck::NoModelListing => Some(UncheckedKey {
            reason: UncheckedReason::NoModelListing,
            message: "rho lists no models for this provider, so it cannot ask about a key".into(),
        }),
        KeyCheck::Unreachable(message) => Some(UncheckedKey {
            reason: UncheckedReason::Unreachable,
            message,
        }),
        KeyCheck::Forbidden => Some(UncheckedKey {
            reason: UncheckedReason::Forbidden,
            message: "the key may not list the provider's models (HTTP 403 Forbidden), so rho could not ask about it".into(),
        }),
    };
    Ok(AuthKeyResponse { unchecked })
}

/// Deletes `params.auth`'s saved credentials.
pub(super) async fn logout(params: AuthLogoutParams) -> Result<AuthLogoutResponse, RpcError> {
    let AuthLogoutParams { auth, environment } = params;
    let credentials = client_credentials(&environment)?;
    let signed_out = auth.clone();
    off_workers(move || {
        credential_writes::sign_out_named(&auth, &repository(), credentials.as_ref())
    })
    .await??;
    tracing::info!(auth = %signed_out, "signed out");
    Ok(AuthLogoutResponse {})
}

/// Runs `operation`, which blocks on config or the credential store, on
/// tokio's blocking pool.
pub(super) async fn off_workers<T: Send + 'static>(
    operation: impl FnOnce() -> T + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| {
            RpcError::new(
                ErrorCode::InternalError,
                format!("the credential write did not finish: {error}"),
            )
        })
}

impl From<CredentialWriteError> for RpcError {
    fn from(error: CredentialWriteError) -> Self {
        let code = match &error {
            CredentialWriteError::UnknownAuth { .. }
            | CredentialWriteError::NotAnApiKey { .. }
            | CredentialWriteError::NoCredentials { .. }
            | CredentialWriteError::BlankKey => ErrorCode::InvalidParams,
            // A credential in the asking client's environment wins over a
            // saved one in that client's sessions, so the message says where
            // to unset it.
            CredentialWriteError::EnvironmentOverride {
                auth,
                variable,
                write,
            } => {
                let data = serde_json::to_value(rho_protocol::EnvironmentOverride {
                    variable: (*variable).to_owned(),
                })
                .expect("an environment override serializes");
                return RpcError::new(
                    ErrorCode::EnvironmentOverride,
                    format!(
                        "'{auth}' reads its credential from {variable} in this client's environment, which wins over a saved one; unset it there to {write}"
                    ),
                )
                .with_data(data);
            }
            CredentialWriteError::KeyRefused { .. } => ErrorCode::KeyRefused,
            CredentialWriteError::Config(_) => ErrorCode::InternalError,
            CredentialWriteError::Store(_) => ErrorCode::CredentialStoreUnavailable,
        };
        RpcError::new(code, error.to_string())
    }
}
