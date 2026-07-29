//! OpenAI server-side compaction via `POST /responses/compact`.
//!
//! Both Codex and direct API-key OpenAI use the unary compact endpoint. The
//! server returns replacement output items (retained user messages plus one
//! encrypted compaction item). Subsequent compatible turns must use the
//! Responses API so the compaction item can be replayed.

use serde_json::Value;

use crate::model::{ModelError, ModelRequest, ModelUsage};
use crate::protocol::openai_responses::{retained_system_messages, CompactUserRetention};
use crate::providers::native_compaction::{
    native_compact_failure, native_compact_from_response_body,
};

use super::auth::Auth;
use super::codex_request::{build_responses_compact_body, ResponsesProfile};
use super::codex_ws::CodexWsTransport;
use super::reasoning::OpenAiReasoningProfile;
use super::responses_http::{
    ResponsesEndpoint, ResponsesFailedAttempt, ResponsesFailedAttemptKind, ResponsesHttpTransport,
};

/// Portable notice shown when the encrypted compaction artifact cannot replay
/// (model/provider/API switch). Server-returned user messages remain in history.
const PORTABLE_HANDOFF_NOTICE: &str = "\
Context was compacted with OpenAI server-side compaction. Prior assistant replies \
and tool results live in an encrypted artifact that only compatible OpenAI Responses \
turns can read. Retained recent user messages are kept below.";

fn native_failed_attempts(
    attempts: Vec<ResponsesFailedAttempt>,
) -> Vec<rho_sdk::provider::NativeCompactionFailedAttempt> {
    attempts
        .into_iter()
        .map(|attempt| {
            let kind = match attempt.kind {
                ResponsesFailedAttemptKind::Authentication => {
                    rho_sdk::ProviderErrorKind::Authentication
                }
            };
            rho_sdk::provider::NativeCompactionFailedAttempt::new(kind, ModelUsage::default())
        })
        .collect()
}

/// Runs native compaction through the shared Responses HTTP transport.
pub(super) async fn compact_with_http(
    auth: &Auth,
    profile: &ResponsesProfile,
    reasoning_profile: &OpenAiReasoningProfile,
    http: &ResponsesHttpTransport<'_>,
    codex_ws: &CodexWsTransport,
    request: ModelRequest<'_>,
) -> rho_sdk::provider::NativeCompactionResponse {
    let cancellation = request.cancellation.clone();
    let identity = profile.identity().clone();
    // Only system messages are preserved from the source history; capture those
    // alone so the full conversation is not cloned across the HTTP round-trip.
    let retained_system_messages = retained_system_messages(request.messages);
    let body = match build_responses_compact_body(profile, reasoning_profile, request).await {
        Ok(body) => body,
        Err(error) => return native_compact_failure(error, Vec::new()),
    };

    let http_result = http
        .post_json(auth, ResponsesEndpoint::Compact, &body, Some(&cancellation))
        .await;
    let failed_attempts = native_failed_attempts(http_result.failed_attempts);
    let response = match http_result.response {
        Ok(response) => response,
        Err(error) => return native_compact_failure(error, failed_attempts),
    };
    if !response.status().is_success() {
        return native_compact_failure(
            crate::provider_backend::http_error::from_response(response).await,
            failed_attempts,
        );
    }

    let body = tokio::select! {
        result = response.json::<Value>() => match result {
            Ok(body) => body,
            Err(error) => {
                return native_compact_failure(ModelError::from(error), failed_attempts);
            }
        },
        () = cancellation.cancelled() => {
            return native_compact_failure(ModelError::Interrupted, failed_attempts);
        }
    };

    // History shape changed; drop any live previous_response_id baseline.
    if matches!(auth, Auth::Codex { .. }) {
        codex_ws.reset().await;
    }

    native_compact_from_response_body(
        identity,
        &retained_system_messages,
        &body,
        PORTABLE_HANDOFF_NOTICE,
        CompactUserRetention::KeepServerUsers,
        failed_attempts,
    )
}

#[cfg(test)]
#[path = "remote_compaction_tests.rs"]
mod tests;
