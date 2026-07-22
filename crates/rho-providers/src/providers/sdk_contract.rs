//! Shared helpers for exposing application transports through the public SDK
//! provider contract.
//!
//! Built-in providers implement [`rho_sdk::provider::ModelProvider`] directly.
//! Callback-based stream transports remain an internal detail and are bridged
//! here into the SDK's bounded async event sender.

use std::{
    collections::VecDeque,
    future::Future,
    sync::{Arc, Mutex},
};

use rho_sdk::{
    model::{ModelEvent, ModelResponse},
    provider::{ProviderEventSender, ProviderRequestEvent},
    CancellationToken, ProviderError, ProviderErrorKind, Retryability,
};
use tokio::sync::Notify;

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
        | ModelError::MissingGoogleApiKey
        | ModelError::MissingGithubCopilotAuth
        | ModelError::MissingMoonshotApiKey
        | ModelError::MissingOpenRouterApiKey
        | ModelError::MissingCredentialProfile(_)
        | ModelError::MissingKimiAuth
        | ModelError::MissingXaiApiKey
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
        // The broken stream cannot be resumed, but retrying the turn as a
        // fresh request may well succeed.
        ModelError::StreamFailedAfterOutput { message } => ProviderError::new(
            ProviderErrorKind::InvalidResponse,
            "provider stream failed after emitting output",
            Retryability::Retryable,
        )
        .with_diagnostic(sanitize_diagnostic(&message)),
        ModelError::InvalidResponse(details) => ProviderError::new(
            ProviderErrorKind::InvalidResponse,
            "provider returned an invalid response",
            Retryability::Permanent,
        )
        .with_diagnostic(sanitize_diagnostic(&details)),
        ModelError::RetryableInvalidResponse {
            error_type,
            message,
        } => ProviderError::new(
            ProviderErrorKind::InvalidResponse,
            "provider returned a temporary invalid response",
            Retryability::Retryable,
        )
        .with_diagnostic(sanitize_diagnostic(&format!("{error_type}: {message}"))),
        ModelError::ProviderReported {
            kind,
            error_type,
            message,
        } => {
            let (provider_kind, public_message, retryability) = match kind {
                crate::model::ProviderReportedErrorKind::Timeout => (
                    ProviderErrorKind::Timeout,
                    "provider reported a timeout",
                    Retryability::Retryable,
                ),
                crate::model::ProviderReportedErrorKind::RateLimit => (
                    ProviderErrorKind::RateLimit,
                    "provider reported a rate limit",
                    Retryability::Retryable,
                ),
                crate::model::ProviderReportedErrorKind::Unavailable => (
                    ProviderErrorKind::Unavailable,
                    "provider reported a temporary failure",
                    Retryability::Retryable,
                ),
                crate::model::ProviderReportedErrorKind::InvalidResponse => (
                    ProviderErrorKind::InvalidResponse,
                    "provider reported an invalid response",
                    Retryability::Permanent,
                ),
            };
            ProviderError::new(provider_kind, public_message, retryability).with_diagnostic(
                sanitize_diagnostic(&format!("{error_type}: {message}")),
            )
        }
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
        ModelError::HttpStatus { status, body } => {
            let status_code = status.as_u16();
            let (kind, retryability) = match status_code {
                401 | 403 => (ProviderErrorKind::Authentication, Retryability::Permanent),
                408 | 504 => (ProviderErrorKind::Timeout, Retryability::Retryable),
                429 => (ProviderErrorKind::RateLimit, Retryability::Retryable),
                500..=599 => (ProviderErrorKind::Unavailable, Retryability::Retryable),
                _ => (ProviderErrorKind::Other, Retryability::Permanent),
            };
            let error = ProviderError::new(kind, format!("HTTP {status_code}"), retryability);
            if body.is_empty() {
                error
            } else {
                error.with_diagnostic(sanitize_diagnostic(&body))
            }
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

fn sanitize_diagnostic(value: &str) -> String {
    const MAX_BYTES: usize = crate::provider_backend::http_error::MAX_ERROR_BODY_BYTES;

    let mut diagnostic = String::new();
    let mut truncated = false;
    for character in value.chars() {
        let escaped = match character {
            '\n' | '\t' => character.to_string(),
            character if character.is_control() => character.escape_default().to_string(),
            character => character.to_string(),
        };
        if diagnostic.len() + escaped.len() > MAX_BYTES {
            truncated = true;
            break;
        }
        diagnostic.push_str(&escaped);
    }
    if truncated {
        diagnostic.push_str("\n[diagnostic truncated]");
    }
    diagnostic
}

/// Event buffered by the application callback adapter.
#[doc(hidden)]
#[derive(Debug)]
pub enum CallbackEvent {
    Model(ModelEvent),
    Request(ProviderRequestEvent),
}

/// Shared queue used by the callback sinks and [`drive_callback_stream`].
pub type CallbackEventQueue = Arc<Mutex<VecDeque<CallbackEvent>>>;

/// Builds the synchronous callback used by application stream transports.
///
/// Events are buffered temporarily because the callback cannot await. The
/// companion [`drive_callback_stream`] loop drains that buffer through the
/// SDK's bounded event sender before polling the provider again.
pub fn callback_event_sink(
    cancellation: CancellationToken,
    pending: CallbackEventQueue,
    notify: Arc<Notify>,
) -> impl FnMut(ModelEvent) -> Result<(), ModelError> + Send {
    move |event| {
        if cancellation.is_cancelled() {
            return Err(ModelError::Interrupted);
        }
        pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(CallbackEvent::Model(event));
        notify.notify_one();
        Ok(())
    }
}

/// Builds the synchronous physical request callback used by application transports.
pub fn callback_request_event_sink(
    cancellation: CancellationToken,
    pending: CallbackEventQueue,
    notify: Arc<Notify>,
) -> impl FnMut(ProviderRequestEvent) -> Result<(), ModelError> + Send {
    move |event| {
        if cancellation.is_cancelled() {
            return Err(ModelError::Interrupted);
        }
        pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(CallbackEvent::Request(event));
        notify.notify_one();
        Ok(())
    }
}

/// Drains buffered callback events through the bounded SDK event channel and
/// drives the provider future with host backpressure across awaits.
pub async fn drive_callback_stream<Fut>(
    cancellation: CancellationToken,
    events: ProviderEventSender,
    pending: CallbackEventQueue,
    notify: Arc<Notify>,
    provider: Fut,
) -> Result<ModelResponse, ProviderError>
where
    Fut: Future<Output = Result<ModelResponse, ModelError>>,
{
    let mut provider = std::pin::pin!(provider);
    let mut provider_result: Option<Result<ModelResponse, ModelError>> = None;

    loop {
        loop {
            let next = pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front();
            let Some(event) = next else {
                break;
            };
            match event {
                CallbackEvent::Model(event) => events.send(event).await?,
                CallbackEvent::Request(ProviderRequestEvent::RequestAttemptFailed {
                    kind,
                    usage,
                }) => {
                    events.send_request_attempt_failed(kind, usage).await?;
                }
            }
        }

        if let Some(result) = provider_result.take() {
            return result.map_err(provider_error_from_model_error);
        }

        let notified = notify.notified();
        let has_pending = !pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty();
        if has_pending {
            continue;
        }

        tokio::select! {
            biased;
            () = notified => {}
            () = cancellation.cancelled() => {
                return Err(ProviderError::interrupted("provider stream interrupted"));
            }
            result = &mut provider => {
                provider_result = Some(result);
            }
        }
    }
}

/// Implements [`rho_sdk::provider::ModelProvider`] for an application transport
/// that already exposes inherent `model_identity`, `complete_turn`, and
/// `stream_turn` methods.
///
/// Streaming buffers same-poll callback bursts, then applies the SDK event
/// channel's async backpressure before polling the provider again.
#[macro_export]
macro_rules! impl_sdk_model_provider {
    ($provider:ty) => {
        impl ::rho_sdk::provider::ModelProvider for $provider {
            fn cancellation_mode(&self) -> ::rho_sdk::provider::ProviderCancellationMode {
                ::rho_sdk::provider::ProviderCancellationMode::Cooperative
            }

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
                    let cancellation = request.cancellation.clone();
                    let pending = ::std::sync::Arc::new(::std::sync::Mutex::new(
                        ::std::collections::VecDeque::new(),
                    ));
                    let notify = ::std::sync::Arc::new(::tokio::sync::Notify::new());
                    let mut on_event = $crate::providers::sdk_contract::callback_event_sink(
                        cancellation.clone(),
                        ::std::sync::Arc::clone(&pending),
                        ::std::sync::Arc::clone(&notify),
                    );
                    let mut on_request_event =
                        $crate::providers::sdk_contract::callback_request_event_sink(
                            cancellation.clone(),
                            ::std::sync::Arc::clone(&pending),
                            ::std::sync::Arc::clone(&notify),
                        );
                    let provider = self.stream_turn(request, &mut on_event, &mut on_request_event);
                    $crate::providers::sdk_contract::drive_callback_stream(
                        cancellation,
                        events,
                        pending,
                        notify,
                        provider,
                    )
                    .await
                })
            }
        }
    };
}

#[cfg(test)]
#[path = "sdk_contract_tests.rs"]
mod tests;
