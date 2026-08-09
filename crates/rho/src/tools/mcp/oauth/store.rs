//! Persistence for one MCP server's OAuth client registration and tokens.
//!
//! Rho already owns a credential store with an OS-keychain and a file backend,
//! so this is only an adapter onto it. Secrets never reach config, the session
//! report, or diagnostics: the account name is the only thing that identifies
//! the entry.

use std::sync::Arc;

use rho_providers::credentials::CredentialStore as RhoCredentialStore;
use rmcp::transport::auth::{AuthError, CredentialStore as RmcpCredentialStore, StoredCredentials};

/// Namespace for MCP OAuth entries inside Rho's credential-store account space.
const ACCOUNT_PREFIX: &str = "mcp-oauth";

/// Account name holding one server's registration and tokens.
///
/// Server identity is the key because it is what the user names in config and
/// what every diagnostic already prints. Repointing an identity at a different
/// authorization server is handled by rmcp, which discards tokens bound to the
/// previous issuer.
pub(crate) fn account_name(identity: &str) -> String {
    format!("{ACCOUNT_PREFIX}:{identity}")
}

/// Rho's credential store behind the store trait rmcp's `AuthorizationManager`
/// drives during discovery, token exchange, and refresh.
#[derive(Clone)]
pub(crate) struct McpOAuthCredentialStore {
    account: String,
    store: Arc<dyn RhoCredentialStore>,
}

impl McpOAuthCredentialStore {
    pub(crate) fn new(identity: &str, store: Arc<dyn RhoCredentialStore>) -> Self {
        Self {
            account: account_name(identity),
            store,
        }
    }

    /// Read the stored entry, treating unreadable JSON as absent.
    ///
    /// A credential written by an older layout must not brick the server; the
    /// next successful login overwrites it.
    fn read(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let raw = self
            .store
            .get_secret(&self.account)
            .map_err(internal_error)?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        match serde_json::from_str::<StoredCredentials>(&raw) {
            Ok(stored) => Ok(Some(stored)),
            Err(error) => {
                tracing::warn!(
                    account = %self.account,
                    error = %error,
                    "stored MCP OAuth credentials could not be read; re-authorization is required"
                );
                Ok(None)
            }
        }
    }

    fn write(&self, credentials: &StoredCredentials) -> Result<(), AuthError> {
        let raw = serde_json::to_string(credentials).map_err(internal_error)?;
        self.store
            .set_secret(&self.account, &raw)
            .map_err(internal_error)
    }

    fn remove(&self) -> Result<(), AuthError> {
        self.store
            .delete_secret(&self.account)
            .map(|_| ())
            .map_err(internal_error)
    }

    /// Run one store operation off the async runtime. The OS keychain backend
    /// blocks, and token refresh happens on the tool-call path.
    async fn off_runtime<T, F>(&self, operation: F) -> Result<T, AuthError>
    where
        T: Send + 'static,
        F: FnOnce(&Self) -> Result<T, AuthError> + Send + 'static,
    {
        let store = self.clone();
        tokio::task::spawn_blocking(move || operation(&store))
            .await
            .map_err(internal_error)?
    }
}

fn internal_error(error: impl std::fmt::Display) -> AuthError {
    AuthError::InternalError(error.to_string())
}

// rmcp declares `CredentialStore` with `#[async_trait]`, so an implementation
// has to use the same macro. Rho's own async traits still return explicit
// `Send` futures.
#[async_trait::async_trait]
impl RmcpCredentialStore for McpOAuthCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        self.off_runtime(Self::read).await
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        self.off_runtime(move |store| store.write(&credentials))
            .await
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.off_runtime(Self::remove).await
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
