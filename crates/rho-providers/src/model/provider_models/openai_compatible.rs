use reqwest::{StatusCode, Url};

use crate::{
    auth::meta_oauth::API_KEY_MINT_URL,
    auth::meta_token::{ensure_fresh, env_api_key, refresh_locked},
    credentials::{load_meta_tokens, CredentialStore},
    model::{registry::missing_credentials_error, ModelError, ReasoningCapabilities},
    provider::{self, ProviderAuthKind, ProviderModelRefreshKind},
    provider_backend::http_error,
};

use super::{
    kimi_capabilities, provider_models_client, OpenAiModelsResponse, ProviderModel,
    ProviderModelHealth,
};

pub async fn probe_provider_models(
    provider: &str,
    api_base: &Url,
    store: &dyn CredentialStore,
) -> ProviderModelHealth {
    let Some(descriptor) = provider::provider_descriptor(provider) else {
        return ProviderModelHealth::InvalidResponse {
            error: ModelError::UnsupportedProvider(provider.into()).to_string(),
        };
    };
    if !descriptor
        .model_refresh
        .is_some_and(ProviderModelRefreshKind::probes_openai_compatible_models)
    {
        return ProviderModelHealth::InvalidResponse {
            error: format!("provider '{provider}' does not use OpenAI-compatible model discovery"),
        };
    }
    match fetch(
        descriptor,
        descriptor.discovery_auth(store),
        api_base,
        store,
    )
    .await
    {
        Ok(models) if models.is_empty() => ProviderModelHealth::ReachableWithoutModels,
        Ok(models) => ProviderModelHealth::ReachableWithModels {
            model_count: models.len(),
        },
        Err(ModelError::Request(error)) if error.is_connect() || error.is_timeout() => {
            ProviderModelHealth::Unreachable {
                error: error.to_string(),
            }
        }
        Err(error) => ProviderModelHealth::InvalidResponse {
            error: error.to_string(),
        },
    }
}

pub(super) async fn fetch(
    descriptor: &provider::ProviderDescriptor,
    auth: provider::AuthMode,
    api_base: &Url,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let muse_env_key = match auth.auth_kind {
        ProviderAuthKind::MetaOAuth { env_var, .. } => env_api_key(env_var),
        _ => None,
    };
    fetch_with_mint(
        descriptor,
        auth,
        api_base,
        store,
        API_KEY_MINT_URL,
        muse_env_key,
    )
    .await
}

async fn fetch_with_mint(
    descriptor: &provider::ProviderDescriptor,
    auth: provider::AuthMode,
    api_base: &Url,
    store: &dyn CredentialStore,
    mint_url: &str,
    muse_env_key: Option<String>,
) -> Result<Vec<ProviderModel>, ModelError> {
    let client = provider_models_client()?;
    let models_url = Url::parse(&format!(
        "{}/models",
        api_base.as_str().trim_end_matches('/')
    ))
    .map_err(|error| ModelError::InvalidResponse(format!("invalid models URL: {error}")))?;
    let response = if matches!(auth.auth_kind, ProviderAuthKind::MetaOAuth { .. }) {
        meta_models_response(&client, models_url, store, muse_env_key, mint_url).await?
    } else {
        let auth = super::request_auth::load(auth, store, &client).await?;
        let request = super::request_auth::authorize_get(&client, models_url, &auth)?;
        http_error::error_for_status(request.send().await?).await?
    };
    let response: OpenAiModelsResponse = response.json().await.map_err(|error| {
        ModelError::InvalidResponse(format!(
            "invalid OpenAI-compatible models response: {error}"
        ))
    })?;
    let mut models = response
        .data
        .into_iter()
        .map(|model| {
            let reasoning_capabilities = if descriptor.name == "kimi-code" {
                kimi_capabilities::reasoning_capabilities(&model.kimi_reasoning)
            } else {
                ReasoningCapabilities::Unknown
            };
            let model_id = descriptor.canonicalize_model_id(&model.id);
            ProviderModel {
                provider: descriptor.name.into(),
                display_name: model.display_name.unwrap_or_else(|| model_id.clone()),
                context_window: model.context_length.filter(|window| *window > 0),
                model: model_id,
                max_output_tokens: None,
                reasoning_capabilities,
            }
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.model.cmp(&right.model));
    models.dedup_by(|left, right| left.model == right.model);
    Ok(models)
}

/// GET `/models` for Muse.
///
/// A static env key is sent once. A stored session remints when the key is near
/// expiry, and once more after HTTP 401. The assumed 24 hour lifetime is not
/// authoritative; the 401 is.
async fn meta_models_response(
    client: &reqwest::Client,
    models_url: Url,
    store: &dyn CredentialStore,
    env_key: Option<String>,
    mint_url: &str,
) -> Result<reqwest::Response, ModelError> {
    if let Some(key) = env_key.filter(|key| !key.trim().is_empty()) {
        let response = client.get(models_url).bearer_auth(key).send().await?;
        return http_error::error_for_status(response).await;
    }
    let mut tokens =
        load_meta_tokens(store)?.ok_or_else(|| missing_credentials_error("meta-muse"))?;
    ensure_fresh(client, store, &mut tokens, mint_url).await?;
    let response = client
        .get(models_url.clone())
        .bearer_auth(&tokens.api_key)
        .send()
        .await?;
    if response.status() != StatusCode::UNAUTHORIZED {
        return http_error::error_for_status(response).await;
    }
    refresh_locked(client, store, &mut tokens, mint_url).await?;
    let response = client
        .get(models_url)
        .bearer_auth(&tokens.api_key)
        .send()
        .await?;
    http_error::error_for_status(response).await
}

#[cfg(test)]
#[path = "openai_compatible_tests.rs"]
mod tests;
