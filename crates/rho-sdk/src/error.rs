use std::fmt;

use crate::tool::ToolError;

/// Stable top-level SDK error classification.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    InvalidConfiguration { message: String },
    Authentication { message: String },
    Provider(ProviderError),
    Tool(ToolError),
    Persistence { message: String },
    PolicyDenied { message: String },
    RuntimeShutdown,
    SessionBusy,
    Cancelled,
    Interrupted { message: String },
    InvalidHostResponse { message: String },
}

impl Error {
    /// Returns whether retrying the failed operation may succeed unchanged.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Provider(error) if error.is_retryable())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration { message } => {
                write!(formatter, "invalid configuration: {message}")
            }
            Self::Authentication { message } => {
                write!(formatter, "authentication failed: {message}")
            }
            Self::Provider(error) => error.fmt(formatter),
            Self::Tool(error) => error.fmt(formatter),
            Self::Persistence { message } => write!(formatter, "persistence failed: {message}"),
            Self::PolicyDenied { message } => {
                write!(formatter, "policy denied operation: {message}")
            }
            Self::RuntimeShutdown => formatter.write_str("runtime has been shut down"),
            Self::SessionBusy => formatter.write_str("session already has an active run"),
            Self::Cancelled => formatter.write_str("operation cancelled"),
            Self::Interrupted { message } => write!(formatter, "operation interrupted: {message}"),
            Self::InvalidHostResponse { message } => {
                write!(formatter, "invalid host response: {message}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Provider(error) => Some(error),
            Self::Tool(error) => Some(error),
            Self::InvalidConfiguration { .. }
            | Self::Authentication { .. }
            | Self::Persistence { .. }
            | Self::PolicyDenied { .. }
            | Self::RuntimeShutdown
            | Self::SessionBusy
            | Self::Cancelled
            | Self::Interrupted { .. }
            | Self::InvalidHostResponse { .. } => None,
        }
    }
}

impl From<ProviderError> for Error {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

impl From<ToolError> for Error {
    fn from(error: ToolError) -> Self {
        Self::Tool(error)
    }
}

/// Provider failure category independent of a provider's wire protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProviderErrorKind {
    Authentication,
    RateLimit,
    Timeout,
    InvalidResponse,
    Unavailable,
    Interrupted,
    Other,
}

/// Whether retrying an operation unchanged may succeed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Retryability {
    Retryable,
    Permanent,
}

/// Sanitized provider failure exposed to SDK hosts.
///
/// The message and `Debug` output must not include credentials, authorization
/// headers, or raw provider payloads. Provider adapters may attach a bounded
/// diagnostic separately for direct, local display to the user.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderError {
    kind: ProviderErrorKind,
    message: String,
    retryability: Retryability,
    diagnostic: Option<String>,
}

impl ProviderError {
    pub fn new(
        kind: ProviderErrorKind,
        message: impl Into<String>,
        retryability: Retryability,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            retryability,
            diagnostic: None,
        }
    }

    /// Adds bounded provider details intended for direct display to the user.
    ///
    /// Diagnostics may contain provider-returned data. Hosts must not add them
    /// to model context, automated reports, or telemetry.
    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.diagnostic = Some(diagnostic.into());
        self
    }

    pub fn kind(&self) -> ProviderErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns provider details for direct user diagnostics only.
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    pub fn is_retryable(&self) -> bool {
        self.retryability == Retryability::Retryable
    }

    pub fn interrupted(message: impl Into<String>) -> Self {
        Self::new(
            ProviderErrorKind::Interrupted,
            message,
            Retryability::Permanent,
        )
    }
}

impl fmt::Debug for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderError")
            .field("kind", &self.kind)
            .field("message", &self.message)
            .field("retryability", &self.retryability)
            .field("diagnostic_available", &self.diagnostic.is_some())
            .finish()
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "provider failed: {}", self.message)
    }
}

impl std::error::Error for ProviderError {}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
