//! Builds the decision model a config entry names.
//!
//! A feature that can use a decision model names it in its own
//! `[internal_agents.<entry>]` table: a provider, a model, and an auth mode.
//! The provider must be one of [`HOSTS`], the providers whose servers speak
//! the System One API.

use anyhow::Context;
use rho_providers::{
    provider::{provider_descriptor, ProviderAuthKind},
    system_one::{SystemOneLimits, SystemOneModel},
    CredentialStore,
};
use rho_sdk::{decision::DecisionModel, SecretString};

use crate::{config::Config, credential_store::AppCredentialStore};

/// A provider whose server speaks the System One API, with what it accepts.
struct Host {
    provider: &'static str,
    limits: SystemOneLimits,
}

/// Cloudflare Workers AI also serves Clef, but truncates every state to its
/// first 2,048 tokens (measured on clef and clef-flash, whatever the state's
/// shape), which drops the pending call the permission screen judges.
const HOSTS: &[Host] = &[
    Host {
        provider: "ollama",
        limits: SystemOneLimits::OLLAMA,
    },
    Host {
        provider: "typesafe",
        limits: SystemOneLimits::TYPESAFE,
    },
];

/// The host providers, for messages.
const HOST_NAMES: &str = "ollama or typesafe";

/// An unusable decision-model entry. Messages name only configured values,
/// never secrets, so a feature can show them to the user.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error(
        "[internal_agents.{entry}] must name a decision model on provider {HOST_NAMES}, got {configured}"
    )]
    UnsupportedProvider {
        entry: &'static str,
        configured: String,
    },
    #[error("[internal_agents.{entry}] auth `{auth}` is not supported; use {supported}")]
    UnsupportedAuth {
        entry: &'static str,
        auth: String,
        /// The host's auth modes, as `` `a` or `b` ``.
        supported: String,
    },
    #[error("[internal_agents.{entry}]: {message}")]
    MissingApiKey {
        entry: &'static str,
        message: &'static str,
    },
}

/// The decision model `[internal_agents.<entry>]` names, or `None` when the
/// entry is absent. An unusable entry is a [`ConfigError`].
pub(crate) fn resolve(
    config: &Config,
    entry: &'static str,
) -> anyhow::Result<Option<Box<dyn DecisionModel>>> {
    let Some(configured) = config.internal_agent_model(entry) else {
        return Ok(None);
    };
    let (selection, host) = configured
        .rho()
        .and_then(|selection| {
            let host = HOSTS
                .iter()
                .find(|host| host.provider == selection.provider)?;
            Some((selection, host))
        })
        .ok_or_else(|| ConfigError::UnsupportedProvider {
            entry,
            configured: configured.display_reference(),
        })?;
    let api_key = api_key(
        entry,
        host.provider,
        &selection.auth,
        &|name| std::env::var(name).ok(),
        &AppCredentialStore,
    )?;
    let api_base = config
        .resolved_provider_endpoint(host.provider)
        .with_context(|| format!("{} has no API base URL", host.provider))?;
    let model =
        SystemOneModel::new(&api_base, selection.model.clone(), api_key)?.with_limits(host.limits);
    Ok(Some(Box::new(model)))
}

/// The API key for `provider`'s `auth` mode, read the way chat providers
/// read it: a nonblank environment variable, else the credential store.
fn api_key(
    entry: &'static str,
    provider: &str,
    auth: &str,
    env: &dyn Fn(&str) -> Option<String>,
    store: &dyn CredentialStore,
) -> anyhow::Result<Option<SecretString>> {
    let descriptor = provider_descriptor(provider)
        .with_context(|| format!("decision host {provider} is not a registered provider"))?;
    let unsupported = || ConfigError::UnsupportedAuth {
        entry,
        auth: auth.into(),
        supported: descriptor
            .auth_modes()
            .filter(|mode| {
                matches!(
                    mode.auth_kind,
                    ProviderAuthKind::None | ProviderAuthKind::ApiKey { .. }
                )
            })
            .map(|mode| format!("`{}`", mode.id))
            .collect::<Vec<_>>()
            .join(" or "),
    };
    let mode = descriptor.auth_mode(auth).ok_or_else(unsupported)?;
    match mode.auth_kind {
        ProviderAuthKind::None => Ok(None),
        ProviderAuthKind::ApiKey {
            env_var,
            account,
            missing_message,
            ..
        } => {
            if let Some(key) = env(env_var).filter(|key| !key.trim().is_empty()) {
                return Ok(Some(SecretString::new(key)));
            }
            let key = store
                .get_secret(account)?
                .filter(|key| !key.trim().is_empty())
                .ok_or(ConfigError::MissingApiKey {
                    entry,
                    message: missing_message,
                })?;
            Ok(Some(SecretString::new(key)))
        }
        ProviderAuthKind::CodexOAuth { .. }
        | ProviderAuthKind::GithubCopilotDevice { .. }
        | ProviderAuthKind::XaiOAuth { .. }
        | ProviderAuthKind::BearerCredential { .. }
        | ProviderAuthKind::KimiOAuth { .. }
        | ProviderAuthKind::MetaOAuth { .. }
        | ProviderAuthKind::OllamaDeviceKey { .. } => Err(unsupported().into()),
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
