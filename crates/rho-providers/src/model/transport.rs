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

    pub fn is_builder(&self) -> bool {
        self.kind == TransportFailureKind::Builder
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
        Self {
            kind,
            message: error.to_string(),
        }
    }
}
