//! Whether the conversation model's auth was chosen or filled in, and the
//! auth a session on it starts on.
//!
//! An auth someone chose (`model.auth` in `config.toml`, `--auth`, an
//! agent's pin) is the one a session starts on, signed in or not, so a
//! client asks to sign in to it. An auth nothing chose is only the
//! provider's default; a session starts instead on the provider's auth
//! that has credentials ([`Config::settle_inferred_auth`]), which every
//! starting point applies: a new session, a draft, a child, a reopen.

use rho_providers::{
    model::catalog::SelectionAuthContext,
    provider::{self, ProviderDescriptor},
};

use super::Config;

/// Where [`Config::auth`] came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AuthOrigin {
    /// Someone chose it: a session starts on it even without credentials.
    Configured,
    /// Filled in because nothing chose one: the provider's default, until
    /// a session settles it on the auth with credentials.
    #[default]
    Inferred,
}

impl Config {
    /// The auth `config.toml` keeps: the chosen one, or none when it was
    /// filled in, so a write never pins an auth nobody chose.
    pub(crate) fn configured_auth(&self) -> Option<String> {
        match self.auth_origin {
            AuthOrigin::Configured => Some(self.auth.clone()),
            AuthOrigin::Inferred => None,
        }
    }

    /// Runs on `auth`, chosen, which sessions start on as it is.
    pub(crate) fn choose_auth(&mut self, auth: impl Into<String>) {
        self.auth = auth.into();
        self.auth_origin = AuthOrigin::Configured;
    }

    /// Runs on `auth`, filled in for want of a choice.
    pub(crate) fn infer_auth(&mut self, auth: impl Into<String>) {
        self.auth = auth.into();
        self.auth_origin = AuthOrigin::Inferred;
    }

    /// Moves an inferred auth to the one a session starts on with
    /// `available`, the auths that have credentials (see
    /// [`provider_auth_with_credentials`]). A chosen auth stays.
    pub(crate) fn settle_inferred_auth(&mut self, available: &[String]) {
        if self.auth_origin == AuthOrigin::Configured {
            return;
        }
        if let Some(descriptor) = provider::provider_descriptor(&self.provider) {
            self.auth = provider_auth_with_credentials(descriptor, &self.auth, available).into();
        }
    }
}

/// The auth `descriptor` runs on when nothing chose one: of the provider's
/// modes in `available_auths`, the one the selection rules pick with
/// `preferred_auth` first, so a stored key wins over keyless access; the
/// provider's default mode when none is available.
pub(crate) fn provider_auth_with_credentials(
    descriptor: &'static ProviderDescriptor,
    preferred_auth: &str,
    available_auths: &[String],
) -> &'static str {
    let modes: Vec<String> = descriptor
        .auth_modes()
        .map(|mode| mode.id.to_string())
        .collect();
    let selected = SelectionAuthContext {
        current: Some(preferred_auth),
        available: available_auths,
    }
    .select(&modes);
    descriptor
        .auth_mode(&selected)
        .unwrap_or_else(|| descriptor.default_auth())
        .id
}

#[cfg(test)]
#[path = "config_auth_tests.rs"]
mod tests;
