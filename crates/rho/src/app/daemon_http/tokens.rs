//! Bearer tokens accepted by `rho daemon serve --http`, kept in rho's
//! credential store under a dedicated account separate from MCP server tokens.
//!
//! Because raw `rho-protocol` gives full user-level control over the daemon
//! (opening sessions in any workspace, answering approvals, changing config,
//! and stopping/restarting the daemon), daemon HTTP tokens are completely
//! distinct from workspace-scoped `rho mcp token` credentials.

use rho_providers::credentials::CredentialStore;
use serde::{Deserialize, Serialize};

use crate::app::{
    http_guard::{find_by_secret, new_secret, sha256_hex, validate_name},
    mcp_server::token_store::{self, StoredSet},
};

/// Credential-store account for daemon HTTP bearer tokens.
pub(crate) const ACCOUNT: &str = "daemon-http-tokens";

/// Cross-process lock file for daemon HTTP token updates.
const LOCK_FILE: &str = ".rho-daemon-tokens.lock";

/// Prefix of every daemon HTTP bearer secret so leaks are immediately
/// recognizable and distinct from `rho_mcp_` tokens.
const SECRET_PREFIX: &str = "rho_daemon_";

/// Every bearer token `rho daemon serve --http` accepts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DaemonTokenSet {
    pub(crate) tokens: Vec<DaemonTokenRecord>,
}

/// One daemon HTTP token: its name and the hex SHA-256 of its secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DaemonTokenRecord {
    pub(crate) name: String,
    sha256: String,
}

impl StoredSet for DaemonTokenSet {
    const ACCOUNT: &'static str = ACCOUNT;
    const WHAT: &'static str = "daemon HTTP tokens";
    const LOCK_FILE: &'static str = LOCK_FILE;
    const GUARDED: &'static str = "daemon HTTP tokens";
}

impl DaemonTokenSet {
    /// Loads the saved token set from `store`; empty when none exists.
    pub(crate) fn load(store: &dyn CredentialStore) -> anyhow::Result<Self> {
        token_store::load(store)
    }

    /// Gives token `name` a fresh 256-bit secret, creating it if needed, and
    /// returns the secret once. Any previous secret stops working immediately.
    pub(crate) fn rotate(&mut self, name: &str) -> anyhow::Result<String> {
        validate_name("token", name)?;
        let secret = new_secret(SECRET_PREFIX);
        let sha256 = sha256_hex(&secret);
        match self.tokens.iter_mut().find(|token| token.name == name) {
            Some(token) => token.sha256 = sha256,
            None => self.tokens.push(DaemonTokenRecord {
                name: name.to_owned(),
                sha256,
            }),
        }
        Ok(secret)
    }

    /// Removes token `name`; returns whether it existed.
    pub(crate) fn revoke(&mut self, name: &str) -> bool {
        let before = self.tokens.len();
        self.tokens.retain(|token| token.name != name);
        self.tokens.len() != before
    }

    /// Returns the matching token record in constant time across all tokens.
    pub(crate) fn authenticate(&self, presented: &str) -> Option<&DaemonTokenRecord> {
        find_by_secret(&self.tokens, presented, |token| &token.sha256)
    }
}

#[cfg(test)]
#[path = "tokens_tests.rs"]
mod tests;
