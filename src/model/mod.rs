use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::tool::{ToolCall, ToolResult, ToolSpec};

pub mod anthropic;
pub mod catalog;
pub mod context;
pub mod github_copilot;
pub mod image;
pub mod models_dev;
pub mod openai;
pub mod provider;
pub mod provider_models;
pub mod registry;

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
    Image(ImageContent),
    ToolCall(ToolCall),
}

#[derive(Clone, Debug)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    /// Provider-specific prompt cache key metadata.
    ///
    /// Providers must opt in explicitly when their API supports this field. For
    /// now, rho only serializes this for OpenAI-Codex Responses requests.
    pub prompt_cache_key: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ModelResponse {
    Assistant(Vec<ContentBlock>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelUsage {
    /// Uncached input tokens charged at the normal input-token rate.
    ///
    /// Provider adapters that report cached tokens inside their input totals
    /// should subtract cache reads before filling this field and report cached
    /// input separately in `cache_read_tokens`.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub cost_usd_micros: Option<u64>,
}

impl ModelUsage {
    /// Input tokens that were present in the request, including cache hits and writes.
    pub fn total_input_tokens(&self) -> Option<u64> {
        let has_input = self.input_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_write_tokens.is_some();
        let total = self
            .input_tokens
            .unwrap_or_default()
            .saturating_add(self.cache_read_tokens.unwrap_or_default())
            .saturating_add(self.cache_write_tokens.unwrap_or_default());
        has_input.then_some(total)
    }
}

#[derive(Clone, Debug)]
pub enum ModelEvent {
    OutputDelta(String),
    ReasoningDelta(String),
    WebSearch(String),
    Usage(ModelUsage),
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("missing OpenAI API key; run /login openai in the TUI or set OPENAI_API_KEY as a CI/dev override")]
    MissingApiKey,
    #[error("missing Codex OAuth credentials; run /login openai-codex in the TUI or set CODEX_ACCESS_TOKEN as a CI/dev override")]
    MissingCodexAuth,
    #[error("missing Anthropic API key; run /login anthropic in the TUI or set ANTHROPIC_API_KEY as a CI/dev override")]
    MissingAnthropicApiKey,
    #[error("missing GitHub Copilot credentials; run /login github-copilot in the TUI or set GITHUB_COPILOT_TOKEN as a CI/dev override")]
    MissingGithubCopilotAuth,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    ApiKey,
    Codex,
}

pub use anthropic::AnthropicProvider;
pub use context::{estimate_context_usage, ContextUsage, ContextUsageSource};
pub use github_copilot::GitHubCopilotProvider;
pub use image::{image_summary, ImageContent};
pub use models_dev::ModelMetadata;
pub use openai::OpenAiProvider;
pub use provider::{build_provider, UnavailableProvider};
