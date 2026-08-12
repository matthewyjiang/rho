use prost::Message;

use crate::{
    auth::cursor_token::resolve_cursor_access_token,
    credentials::CredentialStore,
    model::{
        provider_models::{self, ProviderModel},
        ModelError, ReasoningCapabilities, ReasoningRequestSource,
    },
    protocol::cursor::{
        catalog_model_id, decode_connect_unary_body, fallback_models, models_from_details,
        CursorEffort, CursorModel, GetUsableModelsRequest, GetUsableModelsResponse,
    },
    provider::CURSOR_AGENT_API_BASE,
    reasoning::ReasoningLevel,
};

use super::{CursorProvider, MODELS_PATH};

pub(crate) async fn fetch_usable_models(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let token = resolve_cursor_access_token(store, &client).await?;
    let models = get_usable_models(&client, &token, CURSOR_AGENT_API_BASE).await?;
    let models = if models.is_empty() {
        fallback_models()
    } else {
        models
    };
    Ok(models
        .into_iter()
        .map(|model| to_provider_model(provider, model))
        .collect())
}

async fn get_usable_models(
    client: &reqwest::Client,
    access_token: &str,
    api_base: &str,
) -> Result<Vec<CursorModel>, ModelError> {
    let url = format!("{}{MODELS_PATH}", api_base.trim_end_matches('/'));
    let response =
        CursorProvider::apply_headers(client.post(url), access_token, /* streaming */ false)
            .body(GetUsableModelsRequest {}.encode_to_vec())
            .send()
            .await?;
    if let Some(error) = super::incompatible_protocol(response.status()) {
        return Err(error);
    }
    if !response.status().is_success() {
        return Err(http_status(response).await);
    }
    let bytes = response.bytes().await?;
    let decoded = GetUsableModelsResponse::decode(bytes.as_ref())
        .ok()
        .or_else(|| {
            decode_connect_unary_body(&bytes)
                .and_then(|payload| GetUsableModelsResponse::decode(payload).ok())
        });
    let Some(decoded) = decoded else {
        return Err(ModelError::InvalidResponse(
            "Cursor GetUsableModels response was not valid protobuf".into(),
        ));
    };
    Ok(models_from_details(&decoded.models))
}

async fn http_status(response: reqwest::Response) -> ModelError {
    crate::provider_backend::http_error::from_response(response).await
}

fn to_provider_model(provider: &str, model: CursorModel) -> ProviderModel {
    let reasoning_capabilities = model.reasoning_capabilities();
    ProviderModel {
        provider: provider.to_string(),
        display_name: model.name,
        model: model.id,
        context_window: Some(model.context_window),
        max_output_tokens: Some(model.max_tokens),
        reasoning_capabilities,
    }
}

/// Map a requested reasoning level onto a Cursor effort suffix when discovery
/// advertised that model as having effort variants.
pub(crate) fn run_effort(model: &str, requested: ReasoningLevel) -> CursorEffort {
    let capabilities = provider_models::cached_provider_model("cursor", model)
        .map(|entry| entry.reasoning_capabilities)
        .or_else(|| {
            fallback_models()
                .into_iter()
                .find(|entry| entry.id == catalog_model_id(model))
                .map(|entry| entry.reasoning_capabilities())
        })
        .unwrap_or(ReasoningCapabilities::NotConfigurable);
    match &capabilities {
        ReasoningCapabilities::Levels(_) => capabilities
            .resolve(requested, ReasoningRequestSource::PersistedOrDefault)
            .effective()
            .map(CursorEffort::Level)
            .unwrap_or(CursorEffort::Unspecified),
        ReasoningCapabilities::NotConfigurable | ReasoningCapabilities::Unknown => {
            CursorEffort::Unspecified
        }
    }
}
