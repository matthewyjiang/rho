use reqwest::{RequestBuilder, Url};

use crate::{
    auth::{
        kimi_oauth::{refresh_kimi_tokens, KimiOAuthError},
        kimi_token::token_is_expiring,
        ollama_device::OllamaDeviceKey,
    },
    credentials::{load_kimi_tokens, save_kimi_tokens, CredentialStore, KimiTokens},
    model::ModelError,
    provider::{self, ProviderAuthKind},
};

#[derive(Clone)]
pub(super) enum ModelRequestAuth {
    None,
    Bearer(String),
    OllamaDevice(OllamaDeviceKey),
}

pub(super) async fn load(
    mode: provider::AuthMode,
    store: &dyn CredentialStore,
    client: &reqwest::Client,
) -> Result<ModelRequestAuth, ModelError> {
    match mode.auth_kind {
        ProviderAuthKind::None => Ok(ModelRequestAuth::None),
        ProviderAuthKind::ApiKey { .. } => Ok(ModelRequestAuth::Bearer(
            crate::auth::provider_credentials::load_api_key_for_mode(mode.auth_kind, store)?,
        )),
        ProviderAuthKind::BearerCredential {
            env_var,
            account,
            missing_message,
            ..
        } => Ok(ModelRequestAuth::Bearer(
            crate::auth::provider_credentials::load_stored_bearer_key(
                env_var,
                account,
                missing_message,
                store,
            )?,
        )),
        ProviderAuthKind::KimiOAuth { .. } => {
            let env_var = mode
                .auth_kind
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
                crate::auth::provider_credentials::load_ollama_device_key(missing_message)?,
            ))
        }
        ProviderAuthKind::CodexOAuth { .. }
        | ProviderAuthKind::GithubCopilotDevice { .. }
        | ProviderAuthKind::XaiOAuth { .. } => Err(ModelError::UnsupportedProvider(format!(
            "auth mode '{}'",
            mode.id
        ))),
    }
}

pub(super) fn authorize_get(
    client: &reqwest::Client,
    url: Url,
    auth: &ModelRequestAuth,
) -> Result<RequestBuilder, ModelError> {
    Ok(match auth {
        ModelRequestAuth::None => client.get(url),
        ModelRequestAuth::Bearer(token) => client.get(url).bearer_auth(token),
        ModelRequestAuth::OllamaDevice(key) => {
            let (url, authorization) = key
                .authorize_request("GET", url)
                .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
            client
                .get(url)
                .header(reqwest::header::AUTHORIZATION, authorization)
        }
    })
}

pub(super) fn authorize_post(
    client: &reqwest::Client,
    url: Url,
    auth: &ModelRequestAuth,
) -> Result<RequestBuilder, ModelError> {
    Ok(match auth {
        ModelRequestAuth::None => client.post(url),
        ModelRequestAuth::Bearer(token) => client.post(url).bearer_auth(token),
        ModelRequestAuth::OllamaDevice(key) => {
            let (url, authorization) = key
                .authorize_request("POST", url)
                .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
            client
                .post(url)
                .header(reqwest::header::AUTHORIZATION, authorization)
        }
    })
}
