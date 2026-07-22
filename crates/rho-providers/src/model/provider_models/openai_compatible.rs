use reqwest::Url;

use crate::{
    auth::{
        kimi_oauth::{refresh_kimi_tokens, KimiOAuthError},
        kimi_token::token_is_expiring,
    },
    credentials::{load_kimi_tokens, save_kimi_tokens, CredentialStore, KimiTokens},
    model::{ModelError, ReasoningCapabilities},
    provider::{self, ProviderAuthKind, ProviderModelRefreshKind},
    provider_backend::http_error,
};

use super::{
    kimi_capabilities, load_api_key_auth, provider_models_client, OpenAiModelsResponse,
    ProviderModel, ProviderModelHealth,
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
    match fetch(descriptor, api_base, store).await {
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
    api_base: &Url,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let client = provider_models_client()?;
    let token = match descriptor.auth_kind {
        ProviderAuthKind::None => None,
        ProviderAuthKind::ApiKey { .. } => Some(load_api_key_auth(descriptor.name, store)?),
        ProviderAuthKind::BearerCredential {
            env_var,
            account,
            missing,
            ..
        } => Some(match std::env::var(env_var) {
            Ok(key) if !key.trim().is_empty() => key,
            _ => store
                .get_secret(account)?
                .filter(|key| !key.trim().is_empty())
                .ok_or_else(|| crate::model::registry::missing_credential_error(missing))?,
        }),
        ProviderAuthKind::KimiOAuth { .. } => {
            let env_var = descriptor
                .auth_kind
                .env_var()
                .expect("Kimi OAuth must declare an environment variable");
            let mut tokens = match std::env::var(env_var) {
                Ok(access_token) if !access_token.trim().is_empty() => KimiTokens {
                    access_token,
                    refresh_token: None,
                    expires_at_unix: None,
                    scope: String::new(),
                    token_type: "Bearer".into(),
                    expires_in: None,
                },
                _ => load_kimi_tokens(store)?.ok_or(ModelError::MissingKimiAuth)?,
            };
            if token_is_expiring(&tokens) {
                let refresh_token = tokens
                    .refresh_token
                    .as_deref()
                    .ok_or(ModelError::MissingKimiAuth)?;
                tokens = refresh_kimi_tokens(&client, refresh_token)
                    .await
                    .map_err(|error| match error {
                        KimiOAuthError::Unauthorized(_) => ModelError::MissingKimiAuth,
                        error => ModelError::InvalidResponse(error.to_string()),
                    })?;
                save_kimi_tokens(store, &tokens)?;
            }
            Some(tokens.access_token)
        }
        _ => return Err(ModelError::UnsupportedProvider(descriptor.name.into())),
    };
    let models_url = Url::parse(&format!(
        "{}/models",
        api_base.as_str().trim_end_matches('/')
    ))
    .map_err(|error| ModelError::InvalidResponse(format!("invalid models URL: {error}")))?;
    let request = client.get(models_url);
    let request = match token {
        Some(token) => request.bearer_auth(token),
        None => request,
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
