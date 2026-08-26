use std::time::Duration;

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
    /// Transport failed before a usable HTTP status arrived.
    #[error("request failed: {0}")]
    Request(#[source] super::TransportError),
    #[error("request failed: HTTP {status}: {body}")]
    HttpStatus {
        status: http::StatusCode,
        body: String,
        /// Parsed from the response `Retry-After` header when present.
        retry_after: Option<Duration>,
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

    pub(crate) fn from_reqwest(error: reqwest::Error) -> Self {
        Self::Request(super::TransportError::from_reqwest(error))
    }

    /// Empty assistant turns are transient: a later attempt often produces
    /// text or tool calls. Permanent classification kills the run on the
    /// first thinking-only or blank completion.
    pub(crate) fn empty_assistant() -> Self {
        Self::RetryableInvalidResponse {
            error_type: "empty_assistant".into(),
            message: "assistant message had no content or tool calls".into(),
        }
    }
}
