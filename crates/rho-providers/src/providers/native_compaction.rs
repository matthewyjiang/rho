//! Shared native-compaction response finalization for Responses providers.
//!
//! Protocol parsing stays in [`crate::protocol::openai_shared::compact`]; this
//! module only wraps parsed history into the SDK native-compaction envelope.

use crate::model::{Message, ModelError, ModelIdentity, ModelUsage};
use crate::protocol::openai_responses::{parse_compact_response, CompactUserRetention};

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
    failed_attempts: Vec<rho_sdk::provider::NativeCompactionFailedAttempt>,
) -> rho_sdk::provider::NativeCompactionResponse {
    match parse_compact_response(
        identity,
        retained_system_messages,
        body,
        portable_handoff_notice,
        user_retention,
    ) {
        Ok((messages, usage)) => native_compact_success(messages, usage, failed_attempts),
        Err(error) => native_compact_failure(error, failed_attempts),
    }
}
