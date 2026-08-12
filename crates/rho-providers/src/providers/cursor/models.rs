use prost::Message;

use crate::{
    auth::{
        cursor_oauth::{refresh_cursor_tokens, CursorOAuthError},
        cursor_token::token_is_expiring,
    },
    credentials::{load_cursor_tokens, save_cursor_tokens, CredentialStore},
    model::{
        provider_models::{self, ProviderModel},
        registry::missing_credentials_error,
        ModelError, ReasoningCapabilities, ReasoningRequestSource,
    },
    protocol::cursor::{
        catalog_model_id, decode_connect_unary_body, fallback_models, models_from_details,
        CursorEffort, CursorModel, GetUsableModelsRequest, GetUsableModelsResponse,
    },
    provider::{self, CURSOR_AGENT_API_BASE},
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
    let token = cursor_access_token(store, &client).await?;
    let models = match get_usable_models(&client, &token, CURSOR_AGENT_API_BASE).await {
        Ok(models) if !models.is_empty() => models,
        _ => fallback_models(),
    };
    Ok(models
        .into_iter()
        .map(|model| to_provider_model(provider, model))
        .collect())
}

async fn cursor_access_token(
    store: &dyn CredentialStore,
    client: &reqwest::Client,
) -> Result<String, ModelError> {
    let env_var = provider::provider_descriptor("cursor")
        .and_then(|descriptor| descriptor.default_auth().auth_kind.env_var())
        .expect("Cursor OAuth must declare an environment variable");
    if let Ok(token) = std::env::var(env_var) {
        if !token.trim().is_empty() {
            return Ok(token);
        }
    }
    let missing = || missing_credentials_error("cursor");
    let mut tokens = load_cursor_tokens(store)?.ok_or_else(missing)?;
    if token_is_expiring(&tokens) {
        let refresh_token = tokens.refresh_token.as_deref().ok_or_else(missing)?;
        tokens =
            refresh_cursor_tokens(client, refresh_token)
                .await
                .map_err(|error| match error {
                    CursorOAuthError::Unauthorized(_) => missing(),
                    error => ModelError::InvalidResponse(error.to_string()),
                })?;
        save_cursor_tokens(store, &tokens)?;
    }
    Ok(tokens.access_token)
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
