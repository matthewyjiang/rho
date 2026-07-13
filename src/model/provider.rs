use std::{fmt, sync::Arc};

use crate::{
    auth::github_copilot_token::GitHubCopilotAuthManager,
    credentials::{load_provider_api_key, OsCredentialStore},
    model::{
        models_dev::{cached_reasoning_effort, cached_reasoning_levels},
        openai::auth::{load_api_key_auth, load_codex_auth},
        registry::{missing_credential_error, provider_runtime, AuthMode, ProviderRuntime},
        AnthropicProvider, DynModelProvider, GitHubCopilotProvider, ModelError, ModelProvider,
        ModelRequest, ModelResponse, OpenAiProvider, XaiProvider,
    },
    provider::{self, ProviderAuthKind},
    reasoning::ReasoningLevel,
};

pub fn build_provider(
    provider: &str,
    model: &str,
    reasoning: ReasoningLevel,
) -> Result<DynModelProvider, ModelError> {
    let supported_reasoning = cached_reasoning_levels(provider, model);
    let reasoning = reasoning.normalize(supported_reasoning.as_deref());
    let reasoning_effort = cached_reasoning_effort(provider, model, reasoning);
    let reasoning_summary = reasoning.summary().map(str::to_string);
    let runtime = provider_runtime(provider)
        .ok_or_else(|| ModelError::UnsupportedProvider(provider.to_string()))?;
    match runtime {
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
            anthropic_max_tokens,
        )) as DynModelProvider),
        ProviderRuntime::GithubCopilot => Ok(Box::new(GitHubCopilotProvider::new(
            model.to_string(),
            GitHubCopilotAuthManager::new(Arc::new(OsCredentialStore)),
        )?) as DynModelProvider),
        ProviderRuntime::Xai => Ok(Box::new(XaiProvider::new(
            model.to_string(),
            Arc::new(OsCredentialStore),
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
    let descriptor = provider::provider_descriptor("anthropic")
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
    load_provider_api_key(&store, descriptor.name)?.ok_or_else(|| missing_credential_error(missing))
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
    async fn send_turn(&self, _request: ModelRequest<'_>) -> Result<ModelResponse, ModelError> {
        Err(clone_model_error(&self.error))
    }
}

fn clone_model_error(error: &ModelError) -> ModelError {
    match error {
        ModelError::MissingApiKey => ModelError::MissingApiKey,
        ModelError::MissingCodexAuth => ModelError::MissingCodexAuth,
        ModelError::MissingAnthropicApiKey => ModelError::MissingAnthropicApiKey,
        ModelError::MissingGithubCopilotAuth => ModelError::MissingGithubCopilotAuth,
        ModelError::MissingXaiAuth => ModelError::MissingXaiAuth,
        ModelError::Credentials(err) => ModelError::Credentials(err.clone()),
        ModelError::UnsupportedProvider(provider) => {
            ModelError::UnsupportedProvider(provider.clone())
        }
        ModelError::InvalidResponse(message) => ModelError::InvalidResponse(message.clone()),
        ModelError::Interrupted => ModelError::Interrupted,
        ModelError::StreamIdleTimeout { timeout } => {
            ModelError::StreamIdleTimeout { timeout: *timeout }
        }
        ModelError::StreamFailedAfterOutput { message } => ModelError::StreamFailedAfterOutput {
            message: message.clone(),
        },
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
