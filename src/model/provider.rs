use std::fmt;

use crate::model::{
    AuthMode, DynModelProvider, ModelError, ModelProvider, ModelRequest, ModelResponse,
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
    let provider = match provider {
        "openai" => OpenAiProvider::new_with_reasoning(
            model.to_string(),
            AuthMode::ApiKey,
            reasoning_effort,
            reasoning_summary,
        ),
        "openai-codex" => OpenAiProvider::new_with_reasoning(
            model.to_string(),
            AuthMode::Codex,
            reasoning_effort,
            reasoning_summary,
        ),
        other => return Err(ModelError::UnsupportedProvider(other.to_string())),
    }?;
    Ok(Box::new(provider))
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
