use std::fmt;
use std::time::Duration;

use crate::tool::ToolError;
use crate::{floor_char_boundary, DIAGNOSTIC_TRUNCATION_MARKER};

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

/// Provider-returned details intended only for direct display to the user.
///
/// Values are bounded to 16 KiB and their `Debug` output is redacted because
/// they may contain user data or secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderDiagnostic(String);

impl ProviderDiagnostic {
    const MAX_BYTES: usize = 16 * 1024;

    pub fn new(diagnostic: impl Into<String>) -> Self {
        let diagnostic = diagnostic.into();
        if diagnostic.len() <= Self::MAX_BYTES {
            return Self(diagnostic);
        }

        let content_bytes = Self::MAX_BYTES - DIAGNOSTIC_TRUNCATION_MARKER.len();
        let boundary = floor_char_boundary(&diagnostic, content_bytes);
        let mut bounded = diagnostic[..boundary].to_owned();
        bounded.push_str(DIAGNOSTIC_TRUNCATION_MARKER);
        Self(bounded)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderDiagnostic([redacted])")
    }
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
    diagnostic: Option<ProviderDiagnostic>,
    /// Provider-supplied wait hint from `Retry-After` or an equivalent field.
    retry_after: Option<Duration>,
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
            retry_after: None,
        }
    }

    /// Adds bounded provider details intended for direct display to the user.
    ///
    /// Diagnostics may contain provider-returned data. Hosts must not add them
    /// to model context, automated reports, or telemetry.
    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.diagnostic = Some(ProviderDiagnostic::new(diagnostic));
        self
    }

    /// Records a provider-supplied wait before the next attempt may succeed.
    pub fn with_retry_after(mut self, retry_after: Duration) -> Self {
        self.retry_after = Some(retry_after);
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
        self.diagnostic.as_ref().map(ProviderDiagnostic::as_str)
    }

    /// Provider-supplied wait before retrying may succeed.
    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after
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
            .field("retry_after", &self.retry_after)
            .finish()
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "provider failed: {}", self.message)
    }
}

impl std::error::Error for ProviderError {}

/// Formats a provider wait hint for user-facing text (`12s`, `5m`, `1h 30m`).
pub fn format_retry_after(delay: Duration) -> String {
    let secs = delay.as_secs();
    if secs == 0 {
        return if delay.is_zero() {
            "now".into()
        } else {
            "1s".into()
        };
    }
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    let rem_secs = secs % 60;
    if minutes < 60 {
        return if rem_secs == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {rem_secs}s")
        };
    }
    let hours = minutes / 60;
    let rem_minutes = minutes % 60;
    if rem_minutes == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h {rem_minutes}m")
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
