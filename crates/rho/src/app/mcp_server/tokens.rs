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
use sha2::{Digest, Sha256};

use crate::{
    app::session_control::{Grant, Holder},
    caller_context::{Access, Scope, Workspaces},
};

/// The credential-store account that holds the token set.
pub(crate) const ACCOUNT: &str = "mcp-serve-tokens";

/// Random bytes in a secret: 256 bits, as many as the hash keeps.
const SECRET_BYTES: usize = 32;

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

impl TokenSet {
    /// The set saved in `store`; empty when none is.
    pub(crate) fn load(store: &dyn CredentialStore) -> anyhow::Result<Self> {
        let Some(saved) = store
            .get_secret_fresh(ACCOUNT)
            .context("could not read the MCP server tokens from the credential store")?
        else {
            return Ok(Self::default());
        };
        serde_json::from_str(&saved).context("the saved MCP server tokens do not parse")
    }

    pub(crate) fn save(&self, store: &dyn CredentialStore) -> anyhow::Result<()> {
        let encoded = serde_json::to_string(self)?;
        store
            .set_secret(ACCOUNT, &encoded)
            .context("could not save the MCP server tokens in the credential store")
    }

    /// Gives token `name` a new secret, creating the token if it does not
    /// exist, and returns the secret. Old secrets stop working.
    pub(crate) fn rotate(&mut self, name: &str, rotation: Rotation) -> anyhow::Result<String> {
        validate_name(name)?;
        let workspaces = rotation
            .workspaces
            .iter()
            .map(|path| canonical_dir(path))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let secret = new_secret();
        let sha256 = digest(&secret);
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
        let presented = digest(presented);
        self.tokens.iter().fold(None, |found, token| {
            if constant_time_eq(token.sha256.as_bytes(), presented.as_bytes()) {
                Some(token)
            } else {
                found
            }
        })
    }
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        anyhow::bail!("token name {name:?} must be 1 to 64 ASCII letters, digits, '-', or '_'");
    }
    Ok(())
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

fn new_secret() -> String {
    use rand::{rand_core::UnwrapErr, rngs::SysRng, Rng};
    let mut bytes = [0_u8; SECRET_BYTES];
    UnwrapErr(SysRng).fill_bytes(&mut bytes);
    format!("{SECRET_PREFIX}{}", hex::encode(bytes))
}

fn digest(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

#[cfg(test)]
#[path = "tokens_tests.rs"]
mod tests;
