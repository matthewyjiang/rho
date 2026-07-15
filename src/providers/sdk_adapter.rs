//! Compatibility adapter from application providers to the public SDK contract.
//!
//! Application transports keep the private callback-based
//! [`crate::model::ModelProvider`] surface used by the current agent. This
//! module exposes those same transports through
//! [`rho_sdk::provider::ModelProvider`] without copying wire-format or HTTP
//! logic.
//!
//! # Streaming fidelity
//!
//! The public SDK streams through a bounded async [`ProviderEventSender`] with
//! backpressure. Application providers still emit through a synchronous
//! `FnMut` callback, and the private trait is `async_trait(?Send)`, so its
//! futures are not `Send`. Bridging those models with mid-stream fidelity would
//! require either:
//!
//! - awaiting a non-`Send` callback future inside a `Send` SDK future, or
//! - blocking on the async channel from inside the sync callback, or
//! - rewriting every transport and protocol stream decoder onto
//!   `ProviderEventSender`.
//!
//! This adapter therefore implements [`ModelProvider::send_turn`] fully and
//! relies on the SDK default [`ModelProvider::send_turn_stream`], which
//! synthesizes ordered `OutputDelta` events from the completed assistant text.
//! Reasoning deltas, tool-call deltas, usage, web-search, and provider-native
//! context events are not forwarded until transports target the SDK event
//! channel natively.
//!
//! # Per-request reasoning
//!
//! SDK orchestration passes [`ModelRequest::reasoning_level`]. Application
//! transports still apply reasoning through construction-time configuration and
//! [`crate::model::ModelProvider::set_reasoning`]. Request-level reasoning is
//! accepted by the adapter but not yet applied by the underlying transports.

use std::{fmt, future::Future, pin::Pin, sync::Arc};

use rho_sdk::{
    model::{ModelIdentity, ModelRequest, ModelResponse},
    provider::{ModelProvider as SdkModelProvider, ProviderFuture},
    ProviderError, ProviderErrorKind, Retryability,
};

use crate::model::ModelError;

/// Explicit `Send` future used by application transports that can be adapted.
pub type AppProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ModelResponse, ModelError>> + Send + 'a>>;

/// Application provider surface that can be exposed through the public SDK trait.
///
/// Implementors must return `Send` futures by calling inherent transport methods
/// rather than the private `async_trait(?Send)` application trait. Streaming is
/// intentionally omitted here; see the module docs.
pub trait AdaptableProvider: Send + Sync {
    /// Exact identity required by the public SDK contract.
    fn model_identity(&self) -> ModelIdentity;

    /// Completes one model turn without streaming intermediate events.
    fn complete_turn<'a>(&'a self, request: ModelRequest<'a>) -> AppProviderFuture<'a>;
}

/// Wraps an [`AdaptableProvider`] as a public [`rho_sdk::provider::ModelProvider`].
pub struct SdkProviderAdapter<P> {
    inner: P,
}

impl<P> SdkProviderAdapter<P> {
    /// Wraps an adaptable application provider.
    pub fn new(inner: P) -> Self {
        Self { inner }
    }

    /// Wraps an adaptable application provider in an `Arc` trait object.
    pub fn shared(inner: P) -> Arc<Self>
    where
        P: AdaptableProvider + 'static,
    {
        Arc::new(Self::new(inner))
    }
}

impl<P: AdaptableProvider> fmt::Debug for SdkProviderAdapter<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SdkProviderAdapter")
            .field("identity", &self.inner.model_identity())
            .finish_non_exhaustive()
    }
}

impl<P: AdaptableProvider> SdkModelProvider for SdkProviderAdapter<P> {
    fn identity(&self) -> ModelIdentity {
        self.inner.model_identity()
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.inner
                .complete_turn(request)
                .await
                .map_err(provider_error_from_model_error)
        })
    }
}

impl AdaptableProvider for crate::providers::anthropic::AnthropicProvider {
    fn model_identity(&self) -> ModelIdentity {
        crate::providers::anthropic::AnthropicProvider::model_identity(self)
    }

    fn complete_turn<'a>(&'a self, request: ModelRequest<'a>) -> AppProviderFuture<'a> {
        Box::pin(crate::providers::anthropic::AnthropicProvider::complete_turn(self, request))
    }
}

impl AdaptableProvider for crate::providers::github_copilot::GitHubCopilotProvider {
    fn model_identity(&self) -> ModelIdentity {
        crate::providers::github_copilot::GitHubCopilotProvider::model_identity(self)
    }

    fn complete_turn<'a>(&'a self, request: ModelRequest<'a>) -> AppProviderFuture<'a> {
        Box::pin(
            crate::providers::github_copilot::GitHubCopilotProvider::complete_turn(self, request),
        )
    }
}

impl AdaptableProvider for crate::providers::openai::OpenAiProvider {
    fn model_identity(&self) -> ModelIdentity {
        crate::providers::openai::OpenAiProvider::model_identity(self)
    }

    fn complete_turn<'a>(&'a self, request: ModelRequest<'a>) -> AppProviderFuture<'a> {
        Box::pin(crate::providers::openai::OpenAiProvider::complete_turn(
            self, request,
        ))
    }
}

impl AdaptableProvider for crate::providers::xai::XaiProvider {
    fn model_identity(&self) -> ModelIdentity {
        crate::providers::xai::XaiProvider::model_identity(self)
    }

    fn complete_turn<'a>(&'a self, request: ModelRequest<'a>) -> AppProviderFuture<'a> {
        Box::pin(crate::providers::xai::XaiProvider::complete_turn(
            self, request,
        ))
    }
}

/// Converts an application [`ModelError`] into a sanitized public [`ProviderError`].
///
/// HTTP response bodies and other transport payloads are omitted so credentials
/// and provider-private content do not enter the SDK error contract.
pub fn provider_error_from_model_error(error: ModelError) -> ProviderError {
    match error {
        ModelError::MissingApiKey
        | ModelError::MissingCodexAuth
        | ModelError::MissingAnthropicApiKey
        | ModelError::MissingGithubCopilotAuth
        | ModelError::MissingXaiAuth => ProviderError::new(
            ProviderErrorKind::Authentication,
            error.to_string(),
            Retryability::Permanent,
        ),
        ModelError::Credentials(message) => ProviderError::new(
            ProviderErrorKind::Authentication,
            format!("credential store error: {message}"),
            Retryability::Permanent,
        ),
        ModelError::Interrupted => ProviderError::interrupted("provider stream interrupted"),
        ModelError::StreamIdleTimeout { timeout } => ProviderError::new(
            ProviderErrorKind::Timeout,
            format!(
                "provider stream received no data for {timeout:?}; the connection may be stale"
            ),
            Retryability::Retryable,
        ),
        ModelError::StreamFailedAfterOutput { message } => ProviderError::new(
            ProviderErrorKind::InvalidResponse,
            message,
            Retryability::Permanent,
        ),
        ModelError::InvalidResponse(message) => ProviderError::new(
            ProviderErrorKind::InvalidResponse,
            message,
            Retryability::Permanent,
        ),
        ModelError::UnsupportedProvider(provider) => ProviderError::new(
            ProviderErrorKind::Other,
            format!("unsupported provider '{provider}'"),
            Retryability::Permanent,
        ),
        ModelError::HttpStatus { status, body: _ } => {
            let status_code = status.as_u16();
            let (kind, retryability) = match status_code {
                401 | 403 => (ProviderErrorKind::Authentication, Retryability::Permanent),
                408 | 504 => (ProviderErrorKind::Timeout, Retryability::Retryable),
                429 => (ProviderErrorKind::RateLimit, Retryability::Retryable),
                500..=599 => (ProviderErrorKind::Unavailable, Retryability::Retryable),
                _ => (ProviderErrorKind::Other, Retryability::Permanent),
            };
            ProviderError::new(kind, format!("HTTP {status_code}"), retryability)
        }
        ModelError::Request(error) => ProviderError::new(
            ProviderErrorKind::Unavailable,
            format!("request failed: {error}"),
            Retryability::Retryable,
        ),
        ModelError::Io(error) => ProviderError::new(
            ProviderErrorKind::Other,
            format!("io error: {error}"),
            Retryability::Retryable,
        ),
    }
}

#[cfg(test)]
#[path = "sdk_adapter_tests.rs"]
mod tests;
