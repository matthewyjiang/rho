//! Fetching the provider model lists rho keeps on disk, and the models.dev
//! catalog. The daemon does it for its clients: on `rho/model/refresh`, and
//! once a sign-in saved a credential, so each client reads the same lists
//! (`daemon_host/models.rs`). Each fetch reports how it went, for the
//! client to show.

use rho_protocol as wire;
use rho_providers::{
    credentials::CredentialStore,
    model::{
        catalog::SelectionAuthContext,
        decision_models::{lists_decision_models, refresh_decision_models_with_store},
        force_refresh_models_dev_catalog,
        provider_models::{refresh_provider_models_with_store, ProviderModelEndpoint},
        ModelError,
    },
    provider::{self, ProviderDescriptor},
};

use crate::config::{Config, ProviderConfigs};

/// The auth a refresh of `descriptor`'s lists runs on: of the provider's
/// modes in `available_auths`, the one the selection rules pick with
/// `preferred_auth` first, so a stored key wins over keyless access; the
/// provider's default mode when none is available.
pub(crate) fn refresh_auth_for_provider(
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

/// The providers a full refresh fetches: each has a model or decision-model
/// list and an available auth mode, and none awaits its endpoint login.
/// Built-in Ollama is skipped until `/login ollama`, so a local server is
/// never probed before that.
fn refreshable_providers(
    available_auths: &[String],
    configs: &ProviderConfigs,
) -> Vec<&'static ProviderDescriptor> {
    provider::providers()
        .into_iter()
        .filter(|descriptor| {
            descriptor.supports_model_refresh() || lists_decision_models(descriptor.name)
        })
        .filter(|descriptor| {
            descriptor
                .auth_modes()
                .any(|mode| available_auths.iter().any(|auth| auth == mode.id))
        })
        .filter(|descriptor| !configs.awaits_endpoint_login(**descriptor))
        .collect()
}

/// Fetches every list of every provider `available_auths` reach, each on
/// the auth [`refresh_auth_for_provider`] picks with `config`'s own auth
/// preferred, then the models.dev catalog.
pub(crate) async fn refresh_all(
    store: &dyn CredentialStore,
    config: &Config,
    available_auths: &[String],
) -> wire::ModelRefreshResponse {
    let mut lists = Vec::new();
    for descriptor in refreshable_providers(available_auths, &config.providers) {
        let auth = refresh_auth_for_provider(descriptor, &config.auth, available_auths);
        lists.extend(refresh_provider(descriptor.name, auth, store, config).await);
    }
    let catalog = match force_refresh_models_dev_catalog().await {
        0 => wire::ModelRefreshOutcome::Failed {
            message: "the download failed".into(),
        },
        written => saved(written),
    };
    // Names this process resolved, or failed to, before the lists changed.
    rho_providers::model::forget_model_display_names();
    wire::ModelRefreshResponse {
        lists,
        catalog: Some(catalog),
    }
}

/// Fetches `provider`'s lists with `auth`'s credential in `store`: its
/// decision models, when it lists them, then its chat models, when it
/// serves a list. A provider rho does not know, or one with neither list,
/// fetches nothing.
pub(crate) async fn refresh_provider(
    provider: &str,
    auth: &str,
    store: &dyn CredentialStore,
    config: &Config,
) -> Vec<wire::ModelListRefresh> {
    let Some(descriptor) = provider::provider_descriptor(provider) else {
        return Vec::new();
    };
    let endpoint = config.resolved_provider_endpoint(provider);
    let mut lists = Vec::new();
    if lists_decision_models(provider) {
        if let Some(api_base) = &endpoint {
            let fetched = refresh_decision_models_with_store(provider, auth, store, api_base).await;
            // A chat host that lists no decision models, as most Ollama
            // servers do, has nothing to report.
            let quiet =
                matches!(&fetched, Ok(models) if models.is_empty()) && descriptor.serves_chat();
            if !quiet {
                lists.push(list_refresh(
                    provider,
                    wire::ModelListKind::Decision,
                    fetched.map(|models| models.len()),
                ));
            }
        }
    }
    if descriptor.supports_model_refresh() {
        let model_endpoint = endpoint.as_ref().map_or(
            ProviderModelEndpoint::ProviderOwned,
            ProviderModelEndpoint::OpenAiCompatible,
        );
        let fetched =
            refresh_provider_models_with_store(provider, auth, store, model_endpoint).await;
        lists.push(list_refresh(
            provider,
            wire::ModelListKind::Chat,
            fetched.map(|refresh| refresh.models.len()),
        ));
    }
    rho_providers::model::forget_model_display_names();
    lists
}

/// One fetch's report: how many entries it saved, or why it failed.
fn list_refresh(
    provider: &str,
    list: wire::ModelListKind,
    fetched: Result<usize, ModelError>,
) -> wire::ModelListRefresh {
    let outcome = match fetched {
        Ok(models) => saved(models),
        Err(error) => wire::ModelRefreshOutcome::Failed {
            message: failure(&error),
        },
    };
    wire::ModelListRefresh {
        provider: provider.to_owned(),
        list,
        outcome,
    }
}

/// A fetch that saved `models` entries.
fn saved(models: usize) -> wire::ModelRefreshOutcome {
    wire::ModelRefreshOutcome::Refreshed {
        models: u32::try_from(models).unwrap_or(u32::MAX),
    }
}

/// Why a fetch failed, as a client is told: an HTTP status alone, since a
/// body may echo the request, and a fixed phrase for a transport error,
/// whose text may quote the URL, where a key can sit percent-encoded beyond
/// redaction's reach (#1662, as `credential_writes` checks a key).
fn failure(error: &ModelError) -> String {
    match error {
        ModelError::HttpStatus { status, .. } => format!("HTTP {status}"),
        ModelError::Request(error) => match error.status() {
            Some(status) => format!("HTTP {status}"),
            None if error.is_connect() || error.is_timeout() => {
                "could not reach the provider".into()
            }
            None => "the request to the provider failed".into(),
        },
        error => error.to_string(),
    }
}

#[cfg(test)]
#[path = "model_refresh_tests.rs"]
mod tests;
