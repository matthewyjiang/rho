pub use rho_sdk::model::{
    AbortedAssistant, AssistantMessage, ContentBlock, ImageContent, Message, ModelEvent,
    ModelIdentity, ModelRequest, ModelResponse, ModelUsage, PartialToolCall, ProviderContextBlock,
    ToolCall, ToolResult, ToolSpec,
};
use thiserror::Error;

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

/// Application-private provider contract retained while providers migrate to
/// the public SDK trait.
#[async_trait::async_trait(?Send)]
pub trait ModelProvider: Send + Sync {
    fn identity(&self) -> Option<ModelIdentity> {
        None
    }

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

        for block in &blocks {
            if let ContentBlock::Text(text) = block {
                on_event(ModelEvent::OutputDelta(text.clone()))?;
            }
        }
        Ok(ModelResponse::Assistant(blocks))
    }
}

pub type DynModelProvider = Box<dyn ModelProvider>;
