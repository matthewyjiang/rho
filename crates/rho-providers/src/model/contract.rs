pub use rho_sdk::model::AbortedAssistant;
pub use rho_sdk::model::{
    AssistantMessage, ContentBlock, ImageContent, Message, ModelEvent, ModelIdentity, ModelRequest,
    ModelResponse, ModelUsage, PartialToolCall, ProviderContextBlock, ToolCall, ToolResult,
    ToolSpec,
};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderReportedErrorKind {
    Timeout,
    RateLimit,
    Unavailable,
    InvalidResponse,
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
    #[error("missing Moonshot API key; run /login moonshot in the TUI or set MOONSHOT_API_KEY as a CI/dev override")]
    MissingMoonshotApiKey,
    #[error("missing OpenRouter API key; run /login openrouter in the TUI or set OPENROUTER_API_KEY as a CI/dev override")]
    MissingOpenRouterApiKey,
    #[error("missing Kimi OAuth credentials; run /login kimi-code or set KIMI_ACCESS_TOKEN as a CI/dev override")]
    MissingKimiAuth,
    #[error(
        "missing xAI API key; run /login xai in the TUI or set XAI_API_KEY as a CI/dev override"
    )]
    MissingXaiApiKey,
    #[error("missing xAI OAuth credentials; run /login xai-oauth in the TUI or set XAI_ACCESS_TOKEN as a CI/dev override")]
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
    #[error("provider reported {error_type}: {message}")]
    ProviderReported {
        kind: ProviderReportedErrorKind,
        error_type: String,
        message: String,
    },
    #[error(
        "provider '{provider}' model '{model}' does not support reasoning level '{requested}'"
    )]
    UnsupportedReasoning {
        provider: &'static str,
        model: String,
        requested: crate::reasoning::ReasoningLevel,
    },
    #[error("unsupported provider '{0}'")]
    UnsupportedProvider(String),
}

impl ModelError {
    pub fn credentials(error: impl std::fmt::Display) -> Self {
        Self::Credentials(error.to_string())
    }
}
