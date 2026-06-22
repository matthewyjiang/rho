use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::tool::{ToolCall, ToolResult, ToolSpec};

pub mod catalog;
pub mod openai;
pub mod provider;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Message {
    System(String),
    User(Vec<ContentBlock>),
    Assistant(Vec<ContentBlock>),
    ToolResult(ToolResult),
}

impl Message {
    pub fn user_text(content: impl Into<String>) -> Self {
        Self::User(vec![ContentBlock::Text(content.into())])
    }

    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self::Assistant(vec![ContentBlock::Text(content.into())])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ContentBlock {
    Text(String),
    ToolCall(ToolCall),
}

#[derive(Clone, Debug)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Clone, Debug)]
pub enum ModelResponse {
    Assistant(Vec<ContentBlock>),
}

#[derive(Clone, Debug)]
pub enum ModelEvent {
    OutputDelta(String),
    ReasoningDelta(String),
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("missing OpenAI API key; run /login openai in the TUI or set OPENAI_API_KEY as a CI/dev override")]
    MissingApiKey,
    #[error("missing Codex OAuth credentials; run /login openai-codex in the TUI or set CODEX_ACCESS_TOKEN as a CI/dev override")]
    MissingCodexAuth,
    #[error("credential store error: {0}")]
    Credentials(#[from] crate::credentials::CredentialError),
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("request failed: HTTP {status}: {body}")]
    HttpStatus {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("provider stream interrupted")]
    Interrupted,
    #[error("provider returned invalid response: {0}")]
    InvalidResponse(String),
    #[error("unsupported provider '{0}'")]
    UnsupportedProvider(String),
}

/// Extension point for model backends that can complete agent turns.
///
/// Implementors should translate `ModelRequest` values into provider-specific
/// API calls and return assistant content or tool calls without mutating the
/// request history.
#[async_trait::async_trait(?Send)]
pub trait ModelProvider: Send + Sync {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError>;

    async fn send_turn_stream(
        &self,
        request: ModelRequest,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        let response = self.send_turn(request).await?;
        let ModelResponse::Assistant(blocks) = response;

        for block in blocks.iter() {
            if let ContentBlock::Text(text) = block {
                on_event(ModelEvent::OutputDelta(text.clone()))?;
            }
        }
        Ok(ModelResponse::Assistant(blocks))
    }
}

pub type DynModelProvider = Box<dyn ModelProvider>;

#[derive(Clone, Debug)]
pub enum AuthMode {
    ApiKey,
    Codex,
}

pub use openai::OpenAiProvider;
pub use provider::{build_provider, reasoning_config_value, UnavailableProvider};
