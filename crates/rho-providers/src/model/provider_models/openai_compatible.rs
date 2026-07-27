use reqwest::Url;

use crate::{
    auth::{
        kimi_oauth::{refresh_kimi_tokens, KimiOAuthError},
        kimi_token::token_is_expiring,
        ollama_device::OllamaDeviceKey,
    },
    credentials::{load_kimi_tokens, save_kimi_tokens, CredentialStore, KimiTokens},
    model::{ModelError, ReasoningCapabilities},
    provider::{self, AuthMode, ProviderAuthKind, ProviderModelRefreshKind},
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
    if descriptor.model_refresh != Some(ProviderModelRefreshKind::OpenAiCompatible) {
        return ProviderModelHealth::InvalidResponse {
            error: format!("provider '{provider}' does not use OpenAI-compatible model discovery"),
        };
    }
    match fetch(descriptor, descriptor.default_auth(), api_base, store).await {
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
    auth: AuthMode,
    api_base: &Url,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let client = provider_models_client()?;
    let auth = load_model_request_auth(auth.auth_kind, store, &client).await?;
    let models_url = Url::parse(&format!(
        "{}/models",
        api_base.as_str().trim_end_matches('/')
    ))
    .map_err(|error| ModelError::InvalidResponse(format!("invalid models URL: {error}")))?;
    let request = match auth {
        ModelRequestAuth::None => client.get(models_url),
        ModelRequestAuth::Bearer(token) => client.get(models_url).bearer_auth(token),
        ModelRequestAuth::OllamaDevice(key) => {
            let (url, authorization) = key
                .authorize_request("GET", models_url)
                .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
            client
                .get(url)
                .header(reqwest::header::AUTHORIZATION, authorization)
        }
    };
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

async fn load_model_request_auth(
    auth_kind: ProviderAuthKind,
    store: &dyn CredentialStore,
    client: &reqwest::Client,
) -> Result<ModelRequestAuth, ModelError> {
    match auth_kind {
        ProviderAuthKind::None => Ok(ModelRequestAuth::None),
        ProviderAuthKind::ApiKey {
            env_var,
            account,
            missing_message,
            ..
        }
        | ProviderAuthKind::BearerCredential {
            env_var,
            account,
            missing_message,
            ..
        } => Ok(ModelRequestAuth::Bearer(match std::env::var(env_var) {
            Ok(key) if !key.trim().is_empty() => key,
            _ => store
                .get_secret(account)?
                .filter(|key| !key.trim().is_empty())
                .ok_or_else(|| crate::model::registry::missing_credential_error(missing_message))?,
        })),
        ProviderAuthKind::KimiOAuth { .. } => {
            let env_var = auth_kind
                .env_var()
                .expect("Kimi OAuth must declare an environment variable");
            let missing = || crate::model::registry::missing_credentials_error("kimi-code");
            let mut tokens = match std::env::var(env_var) {
                Ok(access_token) if !access_token.trim().is_empty() => KimiTokens {
                    access_token,
                    refresh_token: None,
                    expires_at_unix: None,
                    scope: String::new(),
                    token_type: "Bearer".into(),
                    expires_in: None,
                },
                _ => load_kimi_tokens(store)?.ok_or_else(missing)?,
            };
            if token_is_expiring(&tokens) {
                let refresh_token = tokens.refresh_token.as_deref().ok_or_else(missing)?;
                tokens = refresh_kimi_tokens(client, refresh_token)
                    .await
                    .map_err(|error| match error {
                        KimiOAuthError::Unauthorized(_) => missing(),
                        error => ModelError::InvalidResponse(error.to_string()),
                    })?;
                save_kimi_tokens(store, &tokens)?;
            }
            Ok(ModelRequestAuth::Bearer(tokens.access_token))
        }
        ProviderAuthKind::OllamaDeviceKey { missing_message } => {
            Ok(ModelRequestAuth::OllamaDevice(
                OllamaDeviceKey::load_default().map_err(|error| match error {
                    crate::auth::ollama_device::OllamaDeviceError::MissingKey(_) => {
                        crate::model::registry::missing_credential_error(missing_message)
                    }
                    error => ModelError::InvalidResponse(error.to_string()),
                })?,
            ))
        }
        _ => Err(ModelError::UnsupportedProvider("auth mode".into())),
    }
}

enum ModelRequestAuth {
    None,
    Bearer(String),
    OllamaDevice(OllamaDeviceKey),
}
