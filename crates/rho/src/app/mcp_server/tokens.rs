//! The bearer tokens `rho mcp serve --http` accepts, kept in rho's
//! credential store: one entry holds every token's name, access, allowed
//! workspaces, and the SHA-256 of its secret. The secret itself is shown
//! once, by `rho mcp token rotate`, and never stored.
//!
//! Keeping them in the credential store, rather than in config, puts all of
//! a token's state behind the store's protection and adds no config keys.

use std::path::{Path, PathBuf};

use anyhow::Context;
use rho_providers::credentials::CredentialStore;
use serde::{Deserialize, Serialize};

use super::token_store::{self, StoredSet};
use crate::{
    app::{
        http_guard::{
            constant_time_eq, new_secret, sha256_hex, validate_name as guard_validate_name,
        },
        session_control::{Grant, Holder},
    },
    caller_context::{Access, Scope, Workspaces},
};

/// The credential-store account that holds the token set.
pub(crate) const ACCOUNT: &str = "mcp-serve-tokens";

/// Prefix of every secret, so one found in a log or a paste is recognizable.
const SECRET_PREFIX: &str = "rho_mcp_";

/// Every token the HTTP server accepts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TokenSet {
    pub(crate) tokens: Vec<TokenRecord>,
}

/// One token: its name, what it grants, and its secret's hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TokenRecord {
    pub(crate) name: String,
    pub(crate) access: Access,
    /// Canonical directories whose sessions the token reaches, with
    /// everything under them.
    pub(crate) workspaces: Vec<PathBuf>,
    /// Hex SHA-256 of the secret.
    sha256: String,
}

impl TokenRecord {
    pub(crate) fn grant(&self) -> Grant {
        Grant {
            holder: Holder::Token(self.name.clone()),
            scope: Scope {
                access: self.access,
                workspaces: Workspaces::Only(self.workspaces.clone()),
            },
        }
    }
}

/// What `rotate` changes about a token. A field left `None` keeps the
/// token's current value, and a new token needs both.
pub(crate) struct Rotation {
    pub(crate) access: Option<Access>,
    /// Directories the token reaches; empty keeps the current ones.
    pub(crate) workspaces: Vec<PathBuf>,
}

impl StoredSet for TokenSet {
    const ACCOUNT: &'static str = ACCOUNT;
    const WHAT: &'static str = "MCP server tokens";
}

impl TokenSet {
    /// The set saved in `store`; empty when none is.
    pub(crate) fn load(store: &dyn CredentialStore) -> anyhow::Result<Self> {
        token_store::load(store)
    }

    #[cfg(test)]
    pub(crate) fn save(&self, store: &dyn CredentialStore) -> anyhow::Result<()> {
        token_store::save(self, store)
    }

    /// Gives token `name` a new secret, creating the token if it does not
    /// exist, and returns the secret. Old secrets stop working.
    pub(crate) fn rotate(&mut self, name: &str, rotation: Rotation) -> anyhow::Result<String> {
        validate_name("token", name)?;
        let workspaces = rotation
            .workspaces
            .iter()
            .map(|path| canonical_dir(path))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let secret = new_secret(SECRET_PREFIX);
        let sha256 = sha256_hex(&secret);
        match self.tokens.iter_mut().find(|token| token.name == name) {
            Some(token) => {
                token.sha256 = sha256;
                if let Some(access) = rotation.access {
                    token.access = access;
                }
                if !workspaces.is_empty() {
                    token.workspaces = workspaces;
                }
            }
            None => {
                if workspaces.is_empty() {
                    anyhow::bail!(
                        "a new token needs at least one --workspace: the directories whose sessions it may reach"
                    );
                }
                self.tokens.push(TokenRecord {
                    name: name.to_owned(),
                    access: rotation.access.unwrap_or(Access::Read),
                    workspaces,
                    sha256,
                });
            }
        }
        Ok(secret)
    }

    /// Removes token `name`; whether it existed.
    pub(crate) fn revoke(&mut self, name: &str) -> bool {
        let before = self.tokens.len();
        self.tokens.retain(|token| token.name != name);
        self.tokens.len() != before
    }

    /// The token whose secret `presented` is, if any. Compares against
    /// every token, each in constant time, so the time taken does not tell
    /// which matched.
    pub(crate) fn authenticate(&self, presented: &str) -> Option<&TokenRecord> {
        let presented = sha256_hex(presented);
        self.tokens.iter().fold(None, |found, token| {
            if constant_time_eq(token.sha256.as_bytes(), presented.as_bytes()) {
                Some(token)
            } else {
                found
            }
        })
    }
}

/// Refuses a `kind` name (a token's, a webhook's) that refusals and lists
/// could not show plainly.
pub(super) fn validate_name(kind: &str, name: &str) -> anyhow::Result<()> {
    guard_validate_name(kind, name)
}

fn canonical_dir(path: &Path) -> anyhow::Result<PathBuf> {
    let resolved = path
        .canonicalize()
        .with_context(|| format!("workspace {}", path.display()))?;
    if !resolved.is_dir() {
        anyhow::bail!("workspace {} is not a directory", path.display());
    }
    Ok(resolved)
}

#[cfg(test)]
#[path = "tokens_tests.rs"]
mod tests;
