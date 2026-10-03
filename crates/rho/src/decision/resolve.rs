//! Builds the decision model a config entry names.
//!
//! A feature that can use a decision model names it in its own
//! `[internal_agents.<entry>]` table: a provider, a model, and an auth mode.
//! Only Ollama serves the System One API among Rho's providers, so it is the
//! only host.

use anyhow::Context;
use rho_providers::{
    provider::{provider_descriptor, ProviderAuthKind},
    CredentialStore,
};

use super::{system_one::SystemOneModel, DecisionModel};
use crate::{config::Config, credential_store::AppCredentialStore};

/// The only provider whose server speaks the System One API.
const HOST_PROVIDER: &str = "ollama";

/// An unusable decision-model entry. Messages name only configured values,
/// never secrets, so a feature can show them to the user.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error(
        "[internal_agents.{entry}] must name a decision model on provider {HOST_PROVIDER}, got {configured}"
    )]
    UnsupportedProvider {
        entry: &'static str,
        configured: String,
    },
    #[error(
        "[internal_agents.{entry}] auth `{auth}` is not supported; use `none` or `ollama-api-key`"
    )]
    UnsupportedAuth { entry: &'static str, auth: String },
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
    let selection = configured
        .rho()
        .filter(|selection| selection.provider == HOST_PROVIDER)
        .ok_or_else(|| ConfigError::UnsupportedProvider {
            entry,
            configured: configured.display_reference(),
        })?;
    let api_key = api_key(
        entry,
        &selection.auth,
        &|name| std::env::var(name).ok(),
        &AppCredentialStore,
    )?;
    let api_base = config
        .resolved_provider_endpoint(HOST_PROVIDER)
        .context("ollama has no API base URL")?;
    let model = SystemOneModel::new(&api_base, selection.model.clone(), api_key)?;
    Ok(Some(Box::new(model)))
}

/// The API key for Ollama's `auth` mode, read the way the Ollama provider
/// reads it: a nonblank environment variable, else the credential store.
fn api_key(
    entry: &'static str,
    auth: &str,
    env: &dyn Fn(&str) -> Option<String>,
    store: &dyn CredentialStore,
) -> anyhow::Result<Option<String>> {
    let unsupported = || ConfigError::UnsupportedAuth {
        entry,
        auth: auth.into(),
    };
    let mode = provider_descriptor(HOST_PROVIDER)
        .and_then(|descriptor| descriptor.auth_modes().find(|mode| mode.id == auth))
        .ok_or_else(unsupported)?;
    match mode.auth_kind {
        ProviderAuthKind::None => Ok(None),
        ProviderAuthKind::ApiKey {
            env_var,
            account,
            missing_message,
            ..
        } => {
            if let Some(key) = env(env_var).filter(|key| !key.trim().is_empty()) {
                return Ok(Some(key));
            }
            let key = store
                .get_secret(account)?
                .filter(|key| !key.trim().is_empty())
                .ok_or(ConfigError::MissingApiKey {
                    entry,
                    message: missing_message,
                })?;
            Ok(Some(key))
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
