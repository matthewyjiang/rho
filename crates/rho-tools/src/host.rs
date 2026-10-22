//! Shared shapes for operations an embedding host performs for the tools.

use std::{fmt, future::Future, pin::Pin};

/// Future returned by host operations, such as [`crate::HostTextFiles`]
/// reads and [`crate::HostTerminal`] waits.
pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, HostError>> + Send + 'a>>;

/// Why a host operation failed, in the host's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostError(String);

impl HostError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HostError {}
