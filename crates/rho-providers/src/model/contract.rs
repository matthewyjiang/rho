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
    /// Missing API key, OAuth token, or other provider credentials.
    ///
    /// The message is owned by the provider descriptor table so new providers do
    /// not need dedicated error variants.
    #[error("{0}")]
    MissingCredentials(&'static str),
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
    #[error("provider returned retryable invalid response {error_type}: {message}")]
    RetryableInvalidResponse { error_type: String, message: String },
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

    pub fn missing_credentials(message: &'static str) -> Self {
        Self::MissingCredentials(message)
    }
}
