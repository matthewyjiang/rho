//! `rho/auth/key` and `rho/auth/logout`: the daemon writes the credential
//! store its sessions read (#1662).
//!
//! [`crate::app::credential_writes`] does the work, as it does for ACP, so
//! both hosts check and save keys and sign out the same way, against the
//! store the daemon's config selects when the request runs. Every write
//! moves the process's credential generation, so each running session uses
//! a new key, or drops a deleted one, from its next prompt. Config and the
//! OS keyring block, so those steps run off the async workers.
//!
//! A key reaches the daemon only over its private socket. Nothing here logs
//! a request's params, and errors that could quote a key have it removed
//! before they leave [`crate::app::credential_writes`].

use rho_protocol::{
    AuthKeyParams, AuthKeyResponse, AuthLogoutParams, AuthLogoutResponse, ErrorCode, RpcError,
    UncheckedKey, UncheckedReason,
};

use crate::app::{
    config_repository::ConfigRepository,
    credential_writes::{
        self, EnvironmentOverride, KeyCheck, KeyError, KeySave, NamedSignOutError, OpenError,
        ProfileError, SaveKeyError, SignOutError,
    },
};

/// The config the daemon reads, found as a session open finds it.
fn repository() -> ConfigRepository {
    ConfigRepository::new(None)
}

/// Checks `params.key` with its provider and saves it. Only the provider's
/// check runs on the async workers.
pub(super) async fn save_key(params: AuthKeyParams) -> Result<AuthKeyResponse, RpcError> {
    let AuthKeyParams { auth, key } = params;
    let pending = off_workers(move || KeySave::prepare(&auth, &key, repository()))
        .await?
        .map_err(key_error)?;
    let check = pending.check().await.map_err(key_error)?;
    let auth = pending.auth();
    off_workers(move || pending.save())
        .await?
        .map_err(key_error)?;
    tracing::info!(%auth, "saved an API key");
    Ok(AuthKeyResponse {
        unchecked: unchecked(check),
    })
}

/// Deletes `params.auth`'s saved credentials.
pub(super) async fn logout(params: AuthLogoutParams) -> Result<AuthLogoutResponse, RpcError> {
    let AuthLogoutParams { auth } = params;
    let signed_out = auth.clone();
    off_workers(move || credential_writes::sign_out_named(&auth, &repository()))
        .await?
        .map_err(sign_out_error)?;
    tracing::info!(auth = %signed_out, "signed out");
    Ok(AuthLogoutResponse {})
}

/// Runs `operation`, which blocks on config or the credential store, on
/// tokio's blocking pool.
async fn off_workers<T: Send + 'static>(
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

/// Why the key went unchecked, as the wire says it.
fn unchecked(check: KeyCheck) -> Option<UncheckedKey> {
    match check {
        KeyCheck::Accepted => None,
        KeyCheck::NoModelListing => Some(UncheckedKey {
            reason: UncheckedReason::NoModelListing,
            message: "rho lists no models for this provider, so it cannot ask about a key".into(),
        }),
        KeyCheck::Unreachable(message) => Some(UncheckedKey {
            reason: UncheckedReason::Unreachable,
            message,
        }),
    }
}

fn key_error(error: KeyError) -> RpcError {
    match error {
        KeyError::Profile(ProfileError::EnvironmentOverride(environment)) => {
            environment_override(environment, Refused::SignIn)
        }
        KeyError::Profile(ProfileError::Unknown { .. } | ProfileError::NotAnApiKey { .. })
        | KeyError::Blank => RpcError::new(ErrorCode::InvalidParams, error.to_string()),
        KeyError::Save(SaveKeyError::Rejected { .. }) => {
            RpcError::new(ErrorCode::KeyRefused, error.to_string())
        }
        KeyError::Save(SaveKeyError::Save { .. }) => {
            RpcError::new(ErrorCode::CredentialStoreUnavailable, error.to_string())
        }
        KeyError::Open(error) => open_error(error),
        KeyError::RecordLogin(_) => RpcError::new(ErrorCode::InternalError, error.to_string()),
    }
}

fn sign_out_error(error: NamedSignOutError) -> RpcError {
    match error {
        NamedSignOutError::Unknown { .. } | NamedSignOutError::NoCredentials { .. } => {
            RpcError::new(ErrorCode::InvalidParams, error.to_string())
        }
        NamedSignOutError::SignOut(SignOutError::EnvironmentOverride(environment)) => {
            environment_override(environment, Refused::SignOut)
        }
        NamedSignOutError::SignOut(SignOutError::Open(error)) => open_error(error),
        NamedSignOutError::SignOut(SignOutError::Delete { .. }) => {
            RpcError::new(ErrorCode::CredentialStoreUnavailable, error.to_string())
        }
        NamedSignOutError::SignOut(SignOutError::ForgetLogin { .. }) => {
            RpcError::new(ErrorCode::InternalError, error.to_string())
        }
    }
}

/// What an environment override refused.
#[derive(Clone, Copy, Debug)]
enum Refused {
    SignIn,
    SignOut,
}

/// A credential in the daemon's own environment wins over a saved one; the
/// client's environment does not matter, so the message says where to unset
/// it.
fn environment_override(environment: EnvironmentOverride, refused: Refused) -> RpcError {
    let EnvironmentOverride { auth, variable } = environment;
    let to = match refused {
        Refused::SignIn => "sign in with a saved key",
        Refused::SignOut => "sign out",
    };
    let data = serde_json::to_value(rho_protocol::EnvironmentOverride {
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
