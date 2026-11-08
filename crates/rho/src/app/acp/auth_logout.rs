//! ACP `logout`: signs out of the auth profile new sessions use, the way
//! `/logout <provider>` does in the TUI, by deleting its credentials from the
//! store `rho login` saves to. [`crate::app::sign_in`] does the sign-out, as
//! it does for the daemon's `rho/auth/logout`; this maps its errors to ACP's.
//!
//! Providers capture credentials when built, so deleting them alone would
//! leave live sessions signed in. The delete moves the process's credential
//! generation (#1662); a session whose provider read credentials before it
//! rebuilds the provider from the store before its next prompt, which then
//! fails `auth_required` like a refused credential does.

use std::path::PathBuf;

use agent_client_protocol::Error as AcpError;

use crate::app::{
    config_repository::ConfigRepository,
    sign_in::{self, SignOutError, SignOutTarget},
};

/// Deletes auth profile `auth`'s stored credentials, and forgets the login
/// of `provider` when it is logged in through its endpoint (Ollama). `config`
/// is the `--config` override the process started with.
///
/// The store is resolved from config as it is now, as `authenticate` does,
/// so a store chosen by a terminal login after launch is the one cleared.
/// A credential set in the environment refuses with `invalid_request` before
/// anything is deleted: success always means new sessions need a sign-in.
pub(super) fn sign_out(
    provider: &str,
    auth: &str,
    config: Option<PathBuf>,
) -> Result<(), AcpError> {
    let target = SignOutTarget::Selected { provider, auth };
    sign_in::sign_out(target, &ConfigRepository::new(config)).map_err(|error| match error {
        SignOutError::EnvironmentOverride { .. } => {
            AcpError::invalid_request().data(error.to_string())
        }
        // Only a profile a client names can be unknown or keyless; ACP signs
        // out of the one its config selects.
        SignOutError::UnknownAuth { .. } | SignOutError::NoCredentials { .. } => {
            AcpError::invalid_params().data(error.to_string())
        }
        SignOutError::Open(_) | SignOutError::Delete { .. } | SignOutError::ForgetLogin { .. } => {
            AcpError::internal_error().data(error.to_string())
        }
    })
}
