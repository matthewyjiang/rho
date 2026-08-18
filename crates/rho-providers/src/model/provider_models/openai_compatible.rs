use reqwest::Url;

use crate::{
    credentials::CredentialStore,
    model::{ModelError, ReasoningCapabilities},
    provider::{self, ProviderModelRefreshKind},
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
    let client = provider_models_client()?;
    let auth = super::request_auth::load(auth, store, &client).await?;
    let models_url = Url::parse(&format!(
        "{}/models",
        api_base.as_str().trim_end_matches('/')
    ))
    .map_err(|error| ModelError::InvalidResponse(format!("invalid models URL: {error}")))?;
    let request = super::request_auth::authorize_get(&client, models_url, &auth)?;
    let response = http_error::error_for_status(request.send().await?).await?;
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
