//! Discover and cache models that answer typed questions over the System One API.
//!
//! Ollama advertises decision models through native `/api/tags` capabilities;
//! every model returned by TypeSafe's `/models` is a decision model. These lists
//! are separate from chat discovery but share its HTTP authentication, timeout,
//! and SQLite cache location. Only successful refreshes replace cached rows.

use reqwest::Url;
use rusqlite::{params, Connection};
use serde::Deserialize;

use crate::{
    credentials::CredentialStore,
    model::ModelError,
    provider::{self, ProviderId},
};

use super::provider_models::{
    authorized_models_get, ollama::native_root, open_provider_models_cache, provider_models_client,
};

/// Decision models last discovered on `provider`, sorted and deduplicated.
/// Empty when the provider was never refreshed or serves none.
pub fn cached_decision_models(provider: &str) -> Vec<String> {
    let Ok(connection) = open_cache() else {
        return Vec::new();
    };
    let Ok(mut statement) =
        connection.prepare("select model from decision_models where provider = ?1 order by model")
    else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map(params![provider], |row| row.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .unwrap_or_default()
}

/// Lists `provider`'s decision models from its server and replaces the cache.
/// `api_base` is the provider's resolved API base (the same one runtime requests use).
pub async fn refresh_decision_models_with_store(
    provider: &str,
    auth: &str,
    store: &dyn CredentialStore,
    api_base: &Url,
) -> Result<Vec<String>, ModelError> {
    let descriptor = provider::provider_descriptor(provider)
        .ok_or_else(|| ModelError::UnsupportedProvider(provider.into()))?;
    let host = discovery_host(descriptor.id)
        .ok_or_else(|| ModelError::UnsupportedProvider(provider.into()))?;
    let auth_mode = descriptor.auth_mode(auth).ok_or_else(|| {
        ModelError::InvalidResponse(format!(
            "unknown auth mode '{auth}' for provider '{provider}'"
        ))
    })?;
    let endpoint = match host {
        DiscoveryHost::Ollama => native_root(api_base)
            .map(|root| {
                root.join("api/tags")
                    .map(|url| (url, "/api/tags"))
                    .map_err(|_| ModelError::InvalidResponse("invalid Ollama tags URL".into()))
            })
            .transpose()?,
        DiscoveryHost::TypeSafe => {
            let mut url = api_base.clone();
            url.set_path(&format!("{}/models", api_base.path().trim_end_matches('/')));
            url.set_query(None);
            url.set_fragment(None);
            Some((url, "/models"))
        }
    };
    let models = if let Some((url, path)) = endpoint {
        let client = provider_models_client()?;
        let response = authorized_models_get(&client, url, auth_mode, store)
            .await?
            .send()
            .await?;
        let display_name = descriptor.display_name;
        let status = response.status();
        if !status.is_success() {
            return Err(ModelError::InvalidResponse(format!(
                "{display_name} {path} returned {status}"
            )));
        }
        // Decoder errors can quote invalid values, including proxy-echoed secrets.
        let response: ModelsResponse = response.json().await.map_err(|_| {
            ModelError::InvalidResponse(format!("invalid {display_name} {path} response"))
        })?;
        response
            .models
            .into_iter()
            .filter(|model| match host {
                // Unlike chat discovery, no `/api/show` fallback for tags
                // without capabilities: servers that serve decision models
                // postdate capabilities in tags (Ollama 0.35.1 lists
                // `["decision"]` for clef and clef-flash).
                DiscoveryHost::Ollama => {
                    model.capabilities.as_deref().is_some_and(|capabilities| {
                        capabilities
                            .iter()
                            .any(|capability| capability == "decision")
                    })
                }
                DiscoveryHost::TypeSafe => true,
            })
            .map(|model| model.name)
            .collect()
    } else {
        Vec::new()
    };
    let models = normalized_models(models);
    replace_cache(descriptor.name, &models).map_err(cache_error)?;
    Ok(models)
}

/// Whether `provider` can list decision models (ollama, typesafe).
pub fn lists_decision_models(provider: &str) -> bool {
    provider::provider_descriptor(provider)
        .and_then(|descriptor| discovery_host(descriptor.id))
        .is_some()
}

/// Replaces `provider`'s cached decision models through the production write path.
///
/// Uses the provider-model cache directory override when installed by tests.
#[doc(hidden)]
pub fn replace_cached_decision_models_for_tests(provider: &str, models: Vec<String>) {
    replace_cache(provider, &normalized_models(models))
        .expect("decision model test cache should be writable");
}

#[derive(Clone, Copy)]
enum DiscoveryHost {
    Ollama,
    TypeSafe,
}

fn discovery_host(id: ProviderId) -> Option<DiscoveryHost> {
    match id {
        ProviderId::Ollama => Some(DiscoveryHost::Ollama),
        ProviderId::TypeSafe => Some(DiscoveryHost::TypeSafe),
        ProviderId::OllamaCloud
        | ProviderId::OpenAi
        | ProviderId::OpenAiCodex
        | ProviderId::Anthropic
        | ProviderId::Google
        | ProviderId::GithubCopilot
        | ProviderId::Xai
        | ProviderId::Moonshot
        | ProviderId::Poolside
        | ProviderId::OpenRouter
        | ProviderId::KimiCode
        | ProviderId::QwenTokenPlan
        | ProviderId::Meta
        | ProviderId::OpenCodeGo
        | ProviderId::MiniMax
        | ProviderId::OpenAiCompatible => None,
    }
}

#[derive(Deserialize)]
struct ModelsResponse {
    models: Vec<ListedModel>,
}

#[derive(Deserialize)]
struct ListedModel {
    name: String,
    capabilities: Option<Vec<String>>,
}

fn normalized_models(models: Vec<String>) -> Vec<String> {
    let mut models = models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    models
}

fn open_cache() -> rusqlite::Result<Connection> {
    let connection = open_provider_models_cache()?;
    connection.execute_batch(
        "create table if not exists decision_models (
            provider text not null,
            model text not null,
            updated_at integer not null,
            primary key(provider, model)
        );",
    )?;
    Ok(connection)
}

fn replace_cache(provider: &str, models: &[String]) -> rusqlite::Result<()> {
    let mut connection = open_cache()?;
    let tx = connection.transaction()?;
    tx.execute(
        "delete from decision_models where provider = ?1",
        params![provider],
    )?;
    for model in models {
        tx.execute(
            "insert into decision_models (provider, model, updated_at)
             values (?1, ?2, strftime('%s', 'now'))",
            params![provider, model],
        )?;
    }
    tx.commit()
}

fn cache_error(error: rusqlite::Error) -> ModelError {
    ModelError::InvalidResponse(format!("decision model cache error: {error}"))
}

#[cfg(test)]
#[path = "decision_models_tests.rs"]
mod tests;
