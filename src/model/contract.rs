use crate::cancellation::RunCancellation;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub ok: bool,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartialToolCall {
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AbortedAssistant {
    pub content: Vec<ContentBlock>,
    pub reasoning: String,
    pub tool_calls: Vec<PartialToolCall>,
    pub usage: ModelUsage,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Message {
    System(String),
    User(Vec<ContentBlock>),
    Assistant(Vec<ContentBlock>),
    /// Partial assistant output retained when the run is explicitly cancelled.
    AbortedAssistant(AbortedAssistant),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageContent {
    pub data: String,
    pub mime_type: String,
}

#[derive(Clone, Debug)]
pub struct ModelRequest<'a> {
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    pub cancellation: RunCancellation,
    /// Provider-specific prompt cache key metadata.
    ///
    /// Providers must opt in explicitly when their API supports this field.
    pub prompt_cache_key: Option<&'a str>,
}

#[derive(Clone, Debug)]
pub enum ModelResponse {
    Assistant(Vec<ContentBlock>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
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
    #[error("missing xAI OAuth credentials; run /login xai in the TUI or set XAI_ACCESS_TOKEN as a CI/dev override")]
    MissingXaiAuth,
    #[error("credential store error: {0}")]
    Credentials(String),
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
    #[error("provider stream received no data for {timeout:?}; the connection may be stale")]
    StreamIdleTimeout { timeout: std::time::Duration },
    #[error("provider stream failed after emitting output: {message}")]
    StreamFailedAfterOutput { message: String },
    #[error("provider returned invalid response: {0}")]
    InvalidResponse(String),
    #[error("unsupported provider '{0}'")]
    UnsupportedProvider(String),
}

impl ModelError {
    pub fn credentials(error: impl std::fmt::Display) -> Self {
        Self::Credentials(error.to_string())
    }
}

/// Extension point for model backends that can complete agent turns.
///
/// Implementors should translate `ModelRequest` values into provider-specific
/// API calls and return assistant content or tool calls without mutating the
/// request history.
#[async_trait::async_trait(?Send)]
pub trait ModelProvider: Send + Sync {
    fn set_reasoning(&mut self, _reasoning: crate::reasoning::ReasoningLevel) -> bool {
        false
    }

    async fn send_turn(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ModelError>;

    async fn send_turn_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        let response = tokio::select! {
            response = self.send_turn(request) => response?,
            () = cancellation.cancelled() => return Err(ModelError::Interrupted),
        };
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
