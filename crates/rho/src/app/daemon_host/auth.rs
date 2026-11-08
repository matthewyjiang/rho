//! `rho/auth/key` and `rho/auth/logout`: the daemon writes the credential
//! store its sessions read (#1662).
//!
//! [`crate::app::sign_in`] does the work, as it does for ACP, so both hosts
//! check and save keys and sign out the same way, against the store the
//! daemon's config selects when the request runs. Every write moves the
//! process's credential generation, so each running session uses a new key,
//! or drops a deleted one, from its next prompt.
//!
//! A key reaches the daemon only over its private socket. Nothing here logs
//! a request's params, and errors that could quote a key have it removed
//! before they leave [`crate::app::sign_in`].

use rho_protocol::{
    AuthKeyParams, AuthKeyResponse, AuthLogoutParams, AuthLogoutResponse, EnvironmentOverride,
    ErrorCode, RpcError,
};

use crate::app::{
    config_repository::ConfigRepository,
    sign_in::{self, KeyError, OpenError, SignOutError, SignOutTarget},
};

/// The config the daemon reads, found as a session open finds it.
fn repository() -> ConfigRepository {
    ConfigRepository::new(None)
}

/// Checks `params.key` with its provider and saves it.
pub(super) async fn save_key(params: AuthKeyParams) -> Result<AuthKeyResponse, RpcError> {
    let AuthKeyParams { auth, key } = params;
    let check = sign_in::save_api_key(&auth, &key, &repository())
        .await
        .map_err(|error| key_error(&auth, error))?;
    tracing::info!(%auth, "saved an API key");
    Ok(AuthKeyResponse {
        unchecked: check.unchecked_reason(),
    })
}

/// Deletes `params.auth`'s saved credentials.
pub(super) fn logout(params: AuthLogoutParams) -> Result<AuthLogoutResponse, RpcError> {
    let AuthLogoutParams { auth } = params;
    sign_in::sign_out(SignOutTarget::Named(&auth), &repository()).map_err(|error| match error {
        SignOutError::UnknownAuth { .. } | SignOutError::NoCredentials { .. } => {
            RpcError::new(ErrorCode::InvalidParams, error.to_string())
        }
        SignOutError::EnvironmentOverride { auth, variable } => {
            environment_override(&auth, variable, "sign out")
        }
        SignOutError::Open(error) => open_error(error),
        SignOutError::Delete { .. } => {
            RpcError::new(ErrorCode::CredentialStoreUnavailable, error.to_string())
        }
        SignOutError::ForgetLogin { .. } => {
            RpcError::new(ErrorCode::InternalError, error.to_string())
        }
    })?;
    tracing::info!(%auth, "signed out");
    Ok(AuthLogoutResponse {})
}

fn key_error(auth: &str, error: KeyError) -> RpcError {
    match error {
        KeyError::UnknownAuth { .. } | KeyError::NotAnApiKeyAuth { .. } | KeyError::Blank => {
            RpcError::new(ErrorCode::InvalidParams, error.to_string())
        }
        KeyError::EnvironmentOverride { variable, .. } => {
            environment_override(auth, variable, "sign in with a saved key")
        }
        KeyError::Rejected { .. } => RpcError::new(ErrorCode::KeyRefused, error.to_string()),
        KeyError::Open(error) => open_error(error),
        KeyError::Save { .. } => {
            RpcError::new(ErrorCode::CredentialStoreUnavailable, error.to_string())
        }
        KeyError::RecordLogin(_) => RpcError::new(ErrorCode::InternalError, error.to_string()),
    }
}

/// A credential in the daemon's own environment wins over a saved one; the
/// client's environment does not matter, so the message says where to unset
/// it.
fn environment_override(auth: &str, variable: &str, to: &str) -> RpcError {
    let data = serde_json::to_value(EnvironmentOverride {
        variable: variable.to_owned(),
    })
    .expect("an environment override serializes");
    RpcError::new(
        ErrorCode::EnvironmentOverride,
        format!(
            "'{auth}' reads its credential from {variable} in the daemon's environment; unset it there and restart the daemon to {to}"
        ),
    )
    .with_data(data)
}

fn open_error(error: OpenError) -> RpcError {
    let code = match error {
        OpenError::Config(_) => ErrorCode::InternalError,
        OpenError::Store(_) => ErrorCode::CredentialStoreUnavailable,
    };
    RpcError::new(code, error.to_string())
}
