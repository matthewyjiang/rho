use std::{fmt, sync::Arc};

use crate::{
    auth::github_copilot_token::GitHubCopilotAuthManager,
    credentials::{load_provider_api_key, OsCredentialStore},
    model::{
        openai::auth::{load_api_key_auth, load_codex_auth},
        registry::{self, provider_descriptor, ProviderAuthKind, ProviderRuntime},
        AnthropicProvider, AuthMode, DynModelProvider, GitHubCopilotProvider, ModelError,
        ModelProvider, ModelRequest, ModelResponse, OpenAiProvider,
    },
    reasoning::ReasoningLevel,
};

pub fn build_provider(
    provider: &str,
    model: &str,
    reasoning: ReasoningLevel,
) -> Result<DynModelProvider, ModelError> {
    let reasoning_effort = reasoning.effort().map(str::to_string);
    let reasoning_summary = reasoning.summary().map(str::to_string);
    let descriptor = provider_descriptor(provider)
        .ok_or_else(|| ModelError::UnsupportedProvider(provider.to_string()))?;
    match descriptor.runtime {
        ProviderRuntime::OpenAi { auth_mode } => {
            let credential_store = Arc::new(OsCredentialStore);
            let auth = match auth_mode {
                AuthMode::ApiKey => load_api_key_auth(credential_store.as_ref())?,
                AuthMode::Codex => load_codex_auth(credential_store.as_ref())?,
            };
            Ok(Box::new(OpenAiProvider::new_with_auth(
                model.to_string(),
                auth,
                credential_store,
                reasoning_effort,
                reasoning_summary,
            )) as DynModelProvider)
        }
        ProviderRuntime::Anthropic => Ok(Box::new(AnthropicProvider::new(
            model.to_string(),
            load_anthropic_api_key_auth()?,
            anthropic_max_tokens(model),
        )) as DynModelProvider),
        ProviderRuntime::GithubCopilot => Ok(Box::new(GitHubCopilotProvider::new(
            model.to_string(),
            GitHubCopilotAuthManager::new(Arc::new(OsCredentialStore)),
        )?) as DynModelProvider),
    }
}

fn anthropic_max_tokens(model: &str) -> u32 {
    crate::model::provider_models::cached_provider_model("anthropic", model)
        .and_then(|metadata| metadata.max_output_tokens)
        .or_else(|| {
            crate::model::models_dev::cached_model_metadata("anthropic", model)
                .and_then(|metadata| metadata.max_output_tokens)
        })
        .and_then(|tokens| u32::try_from(tokens).ok())
        .unwrap_or(crate::provider_backend::anthropic::DEFAULT_MAX_TOKENS)
}

fn load_anthropic_api_key_auth() -> Result<String, ModelError> {
    let descriptor = registry::provider_descriptor("anthropic")
        .ok_or_else(|| ModelError::UnsupportedProvider("anthropic".into()))?;
    let ProviderAuthKind::ApiKey {
        env_var, missing, ..
    } = descriptor.auth_kind
    else {
        return Err(ModelError::UnsupportedProvider("anthropic".into()));
    };
    if let Ok(key) = std::env::var(env_var) {
        return Ok(key);
    }
    let store = OsCredentialStore;
    load_provider_api_key(&store, descriptor.name)?
        .ok_or_else(|| registry::missing_credential_error(missing))
}

#[derive(Debug)]
pub struct UnavailableProvider {
    error: ModelError,
}

impl UnavailableProvider {
    pub fn new(error: ModelError) -> Self {
        Self { error }
    }
}

#[async_trait::async_trait(?Send)]
impl ModelProvider for UnavailableProvider {
    async fn send_turn(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        Err(clone_model_error(&self.error))
    }
}

fn clone_model_error(error: &ModelError) -> ModelError {
    match error {
        ModelError::MissingApiKey => ModelError::MissingApiKey,
        ModelError::MissingCodexAuth => ModelError::MissingCodexAuth,
        ModelError::MissingAnthropicApiKey => ModelError::MissingAnthropicApiKey,
        ModelError::MissingGithubCopilotAuth => ModelError::MissingGithubCopilotAuth,
        ModelError::Credentials(err) => ModelError::Credentials(err.clone()),
        ModelError::UnsupportedProvider(provider) => {
            ModelError::UnsupportedProvider(provider.clone())
        }
        ModelError::InvalidResponse(message) => ModelError::InvalidResponse(message.clone()),
        ModelError::Interrupted => ModelError::Interrupted,
        ModelError::HttpStatus { status, body } => ModelError::HttpStatus {
            status: *status,
            body: body.clone(),
        },
        ModelError::Io(err) => ModelError::InvalidResponse(err.to_string()),
        ModelError::Request(err) => ModelError::InvalidResponse(err.to_string()),
    }
}

impl fmt::Display for UnavailableProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.error)
    }
}
