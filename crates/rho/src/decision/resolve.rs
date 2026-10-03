//! Resolves the model a decision-model config entry names.
//!
//! A feature that can use a decision model names it in its own
//! `[internal_agents.<entry>]` table: a provider, a model, an auth mode, and a
//! `kind`. A `decision` model must be on one of [`HOSTS`], the providers whose
//! servers speak the System One API. A `text` model is any chat model, asked
//! through the text adapter. Without `kind`, a model on a host is a decision
//! model and any other is text, which is how entries written before `kind`
//! existed read.

use anyhow::Context;
use rho_providers::{
    credentials::auth_has_stored_credentials,
    model::{decision_models::cached_decision_models, provider_models::cached_provider_models},
    provider::{provider_descriptor, ProviderAuthKind, ProviderDescriptor},
    system_one::{SystemOneLimits, SystemOneModel},
    CredentialStore,
};
use rho_sdk::{decision::DecisionModel, SecretString};

use crate::{
    config::{Config, ModelKind, RhoInternalAgentModel},
    credential_store::AppCredentialStore,
};

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
        "[internal_agents.{entry}] kind `decision` needs a model on provider {HOST_NAMES}, got {configured}"
    )]
    NotOnDecisionHost {
        entry: &'static str,
        configured: String,
    },
    #[error("[internal_agents.{entry}] kind `text` needs a chat model, got {configured}")]
    NotAChatModel {
        entry: &'static str,
        configured: String,
    },
    #[error(
        "[internal_agents.{entry}] must name a model on one of Rho's providers, got {configured}"
    )]
    NotRhoRuntime {
        entry: &'static str,
        configured: String,
    },
    #[error(
        "[internal_agents.{entry}] could not start text model {configured}; check its credentials"
    )]
    TextModelUnavailable {
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
    #[error(
        "[internal_agents.{entry}] allow_threshold_percent must be {min} to {max}, got {percent}"
    )]
    AllowThresholdOutOfRange {
        entry: &'static str,
        percent: u8,
        min: u8,
        max: u8,
    },
    #[error("[internal_agents.{entry}]: {message}")]
    MissingApiKey {
        entry: &'static str,
        message: &'static str,
    },
}

/// The model a decision-model entry names.
pub(crate) enum EntryModel<'a> {
    /// A decision model, ready to ask.
    Decision(Box<dyn DecisionModel>),
    /// A chat model the caller builds and asks through the text adapter. Its
    /// provider is known to serve chat.
    Text(&'a RhoInternalAgentModel),
}

/// Whether `selection` is asked as a decision model or as text: its `kind`,
/// else decision on a host and text elsewhere.
pub(crate) fn entry_kind(selection: &RhoInternalAgentModel) -> ModelKind {
    selection.kind.unwrap_or_else(|| {
        if is_decision_host(&selection.provider) {
            ModelKind::Decision
        } else {
            ModelKind::Text
        }
    })
}

/// Whether `provider` serves decision models.
fn is_decision_host(provider: &str) -> bool {
    HOSTS.iter().any(|host| host.provider == provider)
}

/// The discovered decision model on `provider` that `model` names, if any.
pub(crate) fn discovered_decision_model(provider: &str, model: &str) -> Option<String> {
    cached_decision_models(provider)
        .into_iter()
        .find(|listed| names_listed_model(model, listed))
}

/// Whether config's `model` names `listed`. Ollama lists `clef:latest` for a
/// config's `clef`, so an untagged name matches its `:latest` tag.
fn names_listed_model(model: &str, listed: &str) -> bool {
    listed == model || listed.strip_suffix(":latest") == Some(model)
}

/// Why `selection` likely names the wrong kind, judged by its provider and
/// the models discovered there. `None` when it matches, or when nothing was
/// discovered to judge by. A model listed both as a decision model and as a
/// chat model suits either kind.
pub(crate) fn kind_mismatch(selection: &RhoInternalAgentModel) -> Option<String> {
    let RhoInternalAgentModel {
        provider, model, ..
    } = selection;
    let discovered = discovered_decision_model(provider, model).is_some();
    match entry_kind(selection) {
        ModelKind::Decision if !is_decision_host(provider) => {
            Some(format!("{provider} serves no decision models"))
        }
        ModelKind::Decision => (!discovered && !cached_decision_models(provider).is_empty())
            .then(|| format!("{model} is not a decision model on {provider}")),
        ModelKind::Text => {
            let chat = cached_provider_models(provider)
                .iter()
                .any(|listed| names_listed_model(model, &listed.model));
            (discovered && !chat)
                .then(|| format!("{model} on {provider} is a decision model, not a text model"))
        }
    }
}

/// The model `[internal_agents.<entry>]` names, or `None` when the entry is
/// absent. An unusable entry is a [`ConfigError`].
pub(crate) fn resolve<'a>(
    config: &'a Config,
    entry: &'static str,
) -> anyhow::Result<Option<EntryModel<'a>>> {
    let Some(configured) = config.internal_agent_model(entry) else {
        return Ok(None);
    };
    let Some(selection) = configured.rho() else {
        return Err(ConfigError::NotRhoRuntime {
            entry,
            configured: configured.display_reference(),
        }
        .into());
    };
    let host = match entry_kind(selection) {
        ModelKind::Text => {
            // Custom providers resolve only inside their config's scope.
            let _scope = config.providers.thread_scope()?;
            let descriptor = provider_descriptor(&selection.provider)
                .filter(|descriptor| descriptor.serves_chat())
                .ok_or_else(|| ConfigError::NotAChatModel {
                    entry,
                    configured: configured.display_reference(),
                })?;
            if !has_credentials(
                descriptor,
                &selection.auth,
                &|name| std::env::var(name).ok(),
                &AppCredentialStore,
            ) {
                return Err(ConfigError::TextModelUnavailable {
                    entry,
                    configured: configured.display_reference(),
                }
                .into());
            }
            return Ok(Some(EntryModel::Text(selection)));
        }
        ModelKind::Decision => HOSTS
            .iter()
            .find(|host| host.provider == selection.provider)
            .ok_or_else(|| ConfigError::NotOnDecisionHost {
                entry,
                configured: configured.display_reference(),
            })?,
    };
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
    Ok(Some(EntryModel::Decision(Box::new(model))))
}

/// Whether `auth`, one of `descriptor`'s modes, has a credential: a
/// nonblank environment variable, or one stored. A text screen checks this
/// before it is used, so a headless run refuses to start without one.
fn has_credentials(
    descriptor: &ProviderDescriptor,
    auth: &str,
    env: &dyn Fn(&str) -> Option<String>,
    store: &dyn CredentialStore,
) -> bool {
    descriptor.auth_mode(auth).is_some_and(|mode| {
        mode.auth_kind
            .env_var()
            .and_then(env)
            .is_some_and(|value| !value.trim().is_empty())
            || auth_has_stored_credentials(store, mode.id).unwrap_or(false)
    })
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
