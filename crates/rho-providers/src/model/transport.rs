//! Classified HTTP transport failures for the public provider contract.
//!
//! Reqwest stays behind this type so an HTTP client upgrade is not a public
//! `ModelError` event. Classification matches the SDK mapping: timeout,
//! connect, builder, or other.

use thiserror::Error;

/// Why an HTTP transport request failed before a usable status arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportFailureKind {
    Timeout,
    Connect,
    Builder,
    Other,
}

/// HTTP transport failure owned by this crate.
///
/// The display text is the client error as received. Hosts should match
/// [`Self::kind`] rather than parse the message.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct TransportError {
    kind: TransportFailureKind,
    message: String,
    /// The status of a response the client turned into an error, such as
    /// `error_for_status` does.
    status: Option<http::StatusCode>,
    /// The OS error at the root of the failure, such as a refused
    /// connection or a failed name lookup. Unlike `message`, it never holds
    /// the request URL.
    os_cause: Option<String>,
}

impl TransportError {
    pub fn kind(&self) -> TransportFailureKind {
        self.kind
    }

    pub fn is_timeout(&self) -> bool {
        self.kind == TransportFailureKind::Timeout
    }

    pub fn is_connect(&self) -> bool {
        self.kind == TransportFailureKind::Connect
    }

    /// The HTTP status behind this failure, when the client failed a
    /// response it received rather than the connection.
    pub fn status(&self) -> Option<http::StatusCode> {
        self.status
    }

    /// Why the OS failed the request, when an OS error is at its root.
    pub(crate) fn os_cause(&self) -> Option<&str> {
        self.os_cause.as_deref()
    }

    pub(crate) fn from_reqwest(error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            TransportFailureKind::Timeout
        } else if error.is_builder() {
            TransportFailureKind::Builder
        } else if error.is_connect() {
            TransportFailureKind::Connect
        } else {
            TransportFailureKind::Other
        };
        let os_cause =
            std::iter::successors(std::error::Error::source(&error), |cause| cause.source())
                .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
                .last()
                .map(ToString::to_string);
        Self {
            kind,
            message: error.to_string(),
            status: error.status(),
            os_cause,
        }
    }
}
