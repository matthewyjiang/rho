//! ACP `logout`: signs out of the auth profile new sessions use, the way
//! `/logout <provider>` does in the TUI, by deleting its credentials from the
//! store `rho login` saves to.
//!
//! Providers capture credentials when built, so deleting them alone would
//! leave live sessions signed in. Each logout bumps [`Logouts`]; a session
//! whose provider read credentials before the latest logout rebuilds it from
//! the store before its next prompt, which then fails `auth_required` like a
//! refused credential does.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use agent_client_protocol::Error as AcpError;
use rho_providers::{auth::login_dispatch::ProviderAuthentication, provider};

use super::auth_login::{open_credential_store, UnchosenStore};
use crate::credential_store::AppCredentialStore;

/// How many logouts this connection has completed. Shared by the agent, which
/// records each one, and its session hosts, which compare against it.
/// `pub(in crate::app)` only so launch can build [`super::AcpStartup`].
#[derive(Clone, Debug, Default)]
pub(in crate::app) struct Logouts(Arc<AtomicU64>);

/// A [`Logouts`] reading, taken before a provider reads credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LogoutCount(u64);

impl Logouts {
    pub(super) fn count(&self) -> LogoutCount {
        LogoutCount(self.0.load(Ordering::Acquire))
    }

    /// Call once the credentials are deleted, so a provider that read them
    /// first holds an older count.
    pub(super) fn record(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }

    /// Whether a logout finished after `read` was taken.
    pub(super) fn since(&self, read: LogoutCount) -> bool {
        self.count() != read
    }
}

/// Deletes auth profile `auth`'s stored credentials. `config` is the
/// `--config` override the process started with.
///
/// The store is resolved from config as it is now, as `authenticate` does,
/// so a store chosen by a terminal login after launch is the one cleared.
/// With no store chosen that is the OS store, which sessions read by default.
/// A credential set in the environment cannot be removed, so that refuses
/// before anything is deleted: success always means new sessions need a
/// sign-in.
pub(super) fn sign_out(auth: &str, config: Option<PathBuf>) -> Result<(), AcpError> {
    if ProviderAuthentication::has_environment_override(auth) {
        let source = provider::resolve_auth_mode(auth)
            .and_then(|(_, mode)| mode.auth_kind.env_var())
            .unwrap_or("the environment");
        return Err(AcpError::invalid_request().data(format!(
            "'{auth}' reads its credential from {source}; unset it to sign out"
        )));
    }
    open_credential_store(config, UnchosenStore::UseDefault)
        .map_err(|error| AcpError::internal_error().data(format!("{error:#}")))?;
    ProviderAuthentication::delete_credentials(&AppCredentialStore, auth).map_err(|error| {
        AcpError::internal_error().data(format!("could not delete {auth} credentials: {error}"))
    })?;
    Ok(())
}
