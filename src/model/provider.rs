use std::fmt;

use crate::model::{
    registry::{provider_descriptor, ProviderRuntime},
    AnthropicProvider, DynModelProvider, ModelError, ModelProvider, ModelRequest, ModelResponse,
    OpenAiProvider,
};
use crate::reasoning::ReasoningLevel;

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
        ProviderRuntime::OpenAi { auth_mode } => Ok(Box::new(OpenAiProvider::new_with_reasoning(
            model.to_string(),
            auth_mode,
            reasoning_effort,
            reasoning_summary,
        )?) as DynModelProvider),
        ProviderRuntime::Anthropic => {
            Ok(Box::new(AnthropicProvider::new(model.to_string())?) as DynModelProvider)
        }
    }
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
