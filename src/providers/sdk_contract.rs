//! Shared helpers for exposing application transports through the public SDK
//! provider contract.
//!
//! Built-in providers implement [`rho_sdk::provider::ModelProvider`] directly.
//! Callback-based stream transports remain an internal detail and are bridged
//! here into the SDK's bounded async event sender.

use rho_sdk::{ProviderError, ProviderErrorKind, Retryability};

use crate::model::ModelError;

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
        ModelError::Credentials(_) => ProviderError::new(
            ProviderErrorKind::Authentication,
            "credential store operation failed",
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
        ModelError::StreamFailedAfterOutput { message: _ } => ProviderError::new(
            ProviderErrorKind::InvalidResponse,
            "provider stream failed after emitting output",
            Retryability::Permanent,
        ),
        ModelError::InvalidResponse(_) => ProviderError::new(
            ProviderErrorKind::InvalidResponse,
            "provider returned an invalid response",
            Retryability::Permanent,
        ),
        ModelError::UnsupportedReasoning {
            provider,
            model,
            requested,
        } => ProviderError::new(
            ProviderErrorKind::Other,
            format!(
                "provider '{provider}' model '{model}' does not support reasoning level '{requested}'"
            ),
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
        ModelError::Request(_) => ProviderError::new(
            ProviderErrorKind::Unavailable,
            "provider request failed",
            Retryability::Retryable,
        ),
        ModelError::Io(_) => ProviderError::new(
            ProviderErrorKind::Other,
            "provider I/O failed",
            Retryability::Retryable,
        ),
    }
}

/// Implements [`rho_sdk::provider::ModelProvider`] for an application transport
/// that already exposes inherent `model_identity`, `complete_turn`, and
/// `stream_turn` methods.
///
/// Streaming uses a bounded callback bridge. A callback burst that fills the
/// bridge is interrupted rather than buffered without bound.
#[macro_export]
macro_rules! impl_sdk_model_provider {
    ($provider:ty) => {
        impl ::rho_sdk::provider::ModelProvider for $provider {
            fn identity(&self) -> ::rho_sdk::model::ModelIdentity {
                self.model_identity()
            }

            fn send_turn<'a>(
                &'a self,
                request: ::rho_sdk::model::ModelRequest<'a>,
            ) -> ::rho_sdk::provider::ProviderFuture<'a> {
                ::std::boxed::Box::pin(async move {
                    self.complete_turn(request)
                        .await
                        .map_err($crate::providers::sdk_contract::provider_error_from_model_error)
                })
            }

            fn send_turn_stream<'a>(
                &'a self,
                request: ::rho_sdk::model::ModelRequest<'a>,
                events: ::rho_sdk::provider::ProviderEventSender,
            ) -> ::rho_sdk::provider::ProviderFuture<'a> {
                ::std::boxed::Box::pin(async move {
                    let (event_tx, mut event_rx) =
                        ::tokio::sync::mpsc::channel(events.capacity());
                    let mut on_event = move |event| {
                        event_tx
                            .try_send(event)
                            .map_err(|_| $crate::model::ModelError::Interrupted)
                    };
                    let mut provider = ::std::pin::pin!(self.stream_turn(request, &mut on_event));
                    loop {
                        ::tokio::select! {
                            biased;
                            event = event_rx.recv() => {
                                if let Some(event) = event {
                                    events.send(event).await?;
                                }
                            }
                            result = &mut provider => {
                                while let Ok(event) = event_rx.try_recv() {
                                    events.send(event).await?;
                                }
                                return result.map_err(
                                    $crate::providers::sdk_contract::provider_error_from_model_error,
                                );
                            }
                        }
                    }
                })
            }
        }
    };
}

#[cfg(test)]
#[path = "sdk_contract_tests.rs"]
mod tests;
