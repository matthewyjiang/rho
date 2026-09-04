//! Shared native-compaction response finalization for Responses providers.
//!
//! Protocol parsing stays in [`crate::protocol::openai_shared::compact`]; this
//! module wraps a completed compact HTTP result into the SDK envelope.

use crate::model::{Message, ModelError, ModelIdentity, ModelUsage, ProviderContextBlock};
use crate::protocol::openai_responses::{parse_compact_response, CompactUserRetention};

use super::responses_http::ResponsesHttpResult;

/// Builds a native compaction failure, preserving any prior failed attempts.
pub(crate) fn native_compact_failure(
    error: ModelError,
    failed_attempts: Vec<rho_sdk::provider::NativeCompactionFailedAttempt>,
) -> rho_sdk::provider::NativeCompactionResponse {
    rho_sdk::provider::NativeCompactionResponse::failure(
        crate::providers::sdk_contract::provider_error_from_model_error(error),
    )
    .with_failed_attempts(failed_attempts)
}

/// Wraps parsed replacement history as a native compaction success response.
pub(crate) fn native_compact_success(
    messages: Vec<Message>,
    usage: ModelUsage,
    failed_attempts: Vec<rho_sdk::provider::NativeCompactionFailedAttempt>,
) -> rho_sdk::provider::NativeCompactionResponse {
    match rho_sdk::CompactionOutput::with_usage(messages, usage) {
        Ok(output) => rho_sdk::provider::NativeCompactionResponse::success(output)
            .with_failed_attempts(failed_attempts),
        Err(error) => native_compact_failure(
            ModelError::InvalidResponse(error.to_string()),
            failed_attempts,
        ),
    }
}

/// Parses a compact response body and finalizes the native compaction result.
pub(crate) fn native_compact_from_response_body(
    identity: ModelIdentity,
    retained_system_messages: &[Message],
    body: &serde_json::Value,
    portable_handoff_notice: &str,
    user_retention: CompactUserRetention,
    assistant_context: &[ProviderContextBlock],
    failed_attempts: Vec<rho_sdk::provider::NativeCompactionFailedAttempt>,
) -> rho_sdk::provider::NativeCompactionResponse {
    match parse_compact_response(
        identity,
        retained_system_messages,
        body,
        portable_handoff_notice,
        user_retention,
        assistant_context,
    ) {
        Ok((messages, usage)) => native_compact_success(messages, usage, failed_attempts),
        Err(error) => native_compact_failure(error, failed_attempts),
    }
}

/// How a provider wants the compact response body interpreted.
pub(crate) struct CompactParsePolicy<'a> {
    pub(crate) identity: ModelIdentity,
    pub(crate) retained_system_messages: &'a [Message],
    pub(crate) portable_handoff_notice: &'a str,
    pub(crate) user_retention: CompactUserRetention,
    pub(crate) assistant_context: &'a [ProviderContextBlock],
}

/// Finalizes a completed `POST /responses/compact` result.
///
/// The caller owns credential policy and the HTTP post. This function maps
/// failed attempts, cancel-reads the JSON body, and parses.
pub(crate) async fn native_compact_from_http(
    http_result: ResponsesHttpResult,
    cancellation: &rho_sdk::CancellationToken,
    policy: CompactParsePolicy<'_>,
) -> rho_sdk::provider::NativeCompactionResponse {
    let failed_attempts = http_result.native_failed_attempts();
    let response = match http_result.response {
        Ok(response) => response,
        Err(error) => return native_compact_failure(error, failed_attempts),
    };
    if !response.status().is_success() {
        let error = tokio::select! {
            error = crate::provider_backend::http_error::from_response(response) => error,
            () = cancellation.cancelled() => ModelError::Interrupted,
        };
        return native_compact_failure(error, failed_attempts);
    }

    let body = tokio::select! {
        result = response.json::<serde_json::Value>() => match result {
            Ok(body) => body,
            Err(error) => {
                return native_compact_failure(ModelError::from(error), failed_attempts);
            }
        },
        () = cancellation.cancelled() => {
            return native_compact_failure(ModelError::Interrupted, failed_attempts);
        }
    };

    native_compact_from_response_body(
        policy.identity,
        policy.retained_system_messages,
        &body,
        policy.portable_handoff_notice,
        policy.user_retention,
        policy.assistant_context,
        failed_attempts,
    )
}
