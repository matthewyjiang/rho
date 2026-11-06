//! ACP `logout`: signs out of the auth profile new sessions use, the way
//! `/logout <provider>` does in the TUI, by deleting its credentials from the
//! store `rho login` saves to.
//!
//! Providers capture credentials when built, so deleting them alone would
//! leave live sessions signed in. Each logout bumps the connection's
//! [`Logouts`](crate::session_runtime::Logouts); a session whose provider
//! read credentials before the latest logout rebuilds it from the store
//! before its next prompt, which then fails `auth_required` like a refused
//! credential does.

use std::path::PathBuf;

use agent_client_protocol::Error as AcpError;
use rho_providers::{auth::login_dispatch::ProviderAuthentication, provider};

use super::auth_login::open_credential_store;
use crate::{app::config_repository::ConfigRepository, credential_store::AppCredentialStore};

/// Deletes auth profile `auth`'s stored credentials, and forgets the login
/// of `provider` when it is logged in through its endpoint (Ollama), whose
/// login counts with or without a key. `config` is the `--config` override
/// the process started with.
///
/// The store is resolved from config as it is now, as `authenticate` does,
/// so a store chosen by a terminal login after launch is the one cleared.
/// With no store chosen that is the OS store, which sessions read by default.
/// A credential set in the environment cannot be removed, so that refuses
/// before anything is deleted: success always means new sessions need a
/// sign-in.
pub(super) fn sign_out(
    provider: &str,
    auth: &str,
    config: Option<PathBuf>,
) -> Result<(), AcpError> {
    if ProviderAuthentication::has_environment_override(auth) {
        let source = provider::resolve_auth_mode(auth)
            .and_then(|(_, mode)| mode.auth_kind.env_var())
            .unwrap_or("the environment");
        return Err(AcpError::invalid_request().data(format!(
            "'{auth}' reads its credential from {source}; unset it to sign out"
        )));
    }
    open_credential_store(config.clone())
        .map_err(|error| AcpError::internal_error().data(format!("{error:#}")))?;
    ProviderAuthentication::delete_credentials(&AppCredentialStore, auth).map_err(|error| {
        AcpError::internal_error().data(format!("could not delete {auth} credentials: {error}"))
    })?;
    if let Some(descriptor) = provider::provider_descriptor(provider) {
        crate::app::endpoint_login::forget(&ConfigRepository::new(config), *descriptor).map_err(
            |error| {
                AcpError::internal_error()
                    .data(format!("could not remove the {provider} login: {error:#}"))
            },
        )?;
    }
    Ok(())
}
