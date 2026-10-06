//! Antigravity ACP auth state under the Gemini home.
//!
//! agy_acp_server keeps its own sign-in (separate from the `agy` CLI) in
//! `$GEMINI_HOME/antigravity-acp/` (default `~/.gemini`): `settings.json`
//! names the auth method and OAuth methods store a token beside it (on macOS
//! usually in the Keychain instead). With both present the server needs no
//! ACP `authenticate` call. With a method but no token, `session/new` starts
//! an interactive browser sign-in (`_ensure_oauth_logged_in`, up to 300 s),
//! which would hang a background run, so runs check here first and point at
//! `/login antigravity`.
//!
//! Rho reads only the method name and whether token files exist, never their
//! contents, and never queries the Keychain.

use std::path::{Path, PathBuf};

use serde_json::Value;

const GEMINI_HOME_ENV: &str = "GEMINI_HOME";
/// `FORCE_FILE_STORAGE_ENV_VAR` in agy_acp_server 1.3.0.
const FORCE_FILE_STORAGE_ENV: &str = "AGY_ACP_FORCE_FILE_STORAGE";
const ACP_DIR: &str = "antigravity-acp";
const SETTINGS_FILE: &str = "settings.json";
/// `paths.consumer_token_path()` in agy_acp_server 1.3.0.
const PERSONAL_TOKEN_FILE: &str = "acp_token.json";
/// `paths.business_token_path()` in agy_acp_server 1.3.0.
const BUSINESS_TOKEN_FILE: &str = "acp_business_token.json";

/// The auth method `rho login antigravity` signs in with.
pub(crate) const PERSONAL_OAUTH_METHOD: &str = "oauth-personal";
const BUSINESS_OAUTH_METHOD: &str = "oauth-business";

/// Where the server keeps OAuth tokens (`create_default_store` in 1.3.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenStore {
    /// Only the token file beside `settings.json`: Linux, Windows, or
    /// `AGY_ACP_FORCE_FILE_STORAGE` set.
    File,
    /// The macOS Keychain when it works, else the file. A missing file then
    /// proves nothing, so only the method is checked.
    KeychainOrFile,
}

/// Where Antigravity's ACP state lives. Built from the environment once at
/// the edge; tests construct it directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AntigravityHome {
    acp_dir: PathBuf,
    tokens: TokenStore,
}

impl AntigravityHome {
    /// `$GEMINI_HOME` when set and nonempty (a leading `~` expanded, as the
    /// server does), else `<home>/.gemini`.
    pub(crate) fn resolve(gemini_home: Option<PathBuf>, home: &Path, tokens: TokenStore) -> Self {
        let root = match gemini_home.filter(|value| !value.as_os_str().is_empty()) {
            Some(root) => match root.strip_prefix("~") {
                Ok(rest) => home.join(rest),
                Err(_) => root,
            },
            None => home.join(".gemini"),
        };
        Self {
            acp_dir: root.join(ACP_DIR),
            tokens,
        }
    }

    /// The only environment-reading seam; the caller supplies resolved home.
    pub(crate) fn from_env(home: &Path) -> Self {
        let force_file = std::env::var(FORCE_FILE_STORAGE_ENV)
            .is_ok_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
        let tokens = if cfg!(target_os = "macos") && !force_file {
            TokenStore::KeychainOrFile
        } else {
            TokenStore::File
        };
        Self::resolve(
            std::env::var_os(GEMINI_HOME_ENV).map(PathBuf::from),
            home,
            tokens,
        )
    }

    pub(crate) fn acp_dir(&self) -> &Path {
        &self.acp_dir
    }

    pub(crate) fn settings_path(&self) -> PathBuf {
        self.acp_dir.join(SETTINGS_FILE)
    }

    /// Read the configured method and whether its token exists.
    pub(crate) fn status(&self) -> AntigravityAuthStatus {
        let settings = self.settings_path();
        let raw = match std::fs::read_to_string(&settings) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return AntigravityAuthStatus::SignedOut { settings }
            }
            Err(error) => {
                return AntigravityAuthStatus::Unreadable {
                    settings,
                    detail: error.to_string(),
                }
            }
        };
        // The server rewrites settings.json as standard JSON after sign-in.
        let method = match serde_json::from_str::<Value>(&raw) {
            Ok(value) => value
                .pointer("/auth/type")
                .and_then(Value::as_str)
                .map(str::to_owned),
            Err(error) => {
                return AntigravityAuthStatus::Unreadable {
                    settings,
                    detail: error.to_string(),
                }
            }
        };
        let Some(method) = method.filter(|method| !method.is_empty()) else {
            return AntigravityAuthStatus::SignedOut { settings };
        };
        let token = match method.as_str() {
            PERSONAL_OAUTH_METHOD => Some(PERSONAL_TOKEN_FILE),
            BUSINESS_OAUTH_METHOD => Some(BUSINESS_TOKEN_FILE),
            // API-key, Vertex, and gateway methods read the environment.
            _ => None,
        };
        match (token.map(|file| self.acp_dir.join(file)), self.tokens) {
            (Some(token), TokenStore::File) if !token.is_file() => {
                AntigravityAuthStatus::MissingToken { method, token }
            }
            (Some(_) | None, TokenStore::File | TokenStore::KeychainOrFile) => {
                AntigravityAuthStatus::Configured { method }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AntigravityAuthStatus {
    /// A method is set and any token file it needs exists (or may be in the
    /// Keychain). The server may still reject an expired or revoked token,
    /// or start a browser sign-in for a missing Keychain item, at
    /// `session/new`; the 10 s handshake budget bounds that.
    Configured {
        method: String,
    },
    SignedOut {
        settings: PathBuf,
    },
    MissingToken {
        method: String,
        token: PathBuf,
    },
    Unreadable {
        settings: PathBuf,
        detail: String,
    },
}

impl AntigravityAuthStatus {
    pub(crate) fn is_signed_in(&self) -> bool {
        matches!(self, Self::Configured { .. })
    }

    /// Fail a run before spawning unless signed in.
    pub(crate) fn require_signed_in(&self) -> Result<(), String> {
        let hint = "run `/login antigravity` or `rho login antigravity`";
        match self {
            Self::Configured { .. } => Ok(()),
            Self::SignedOut { settings } => Err(format!(
                "antigravity: not signed in (no auth method in {}); {hint}",
                crate::paths::display(settings)
            )),
            Self::MissingToken { method, token } => Err(format!(
                "antigravity: {method} is selected but {} is missing; {hint}",
                crate::paths::display(token)
            )),
            Self::Unreadable { settings, detail } => Err(format!(
                "antigravity: could not read {}: {detail}",
                crate::paths::display(settings)
            )),
        }
    }
}

#[cfg(test)]
#[path = "home_tests.rs"]
mod tests;
