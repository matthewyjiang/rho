//! xAI native server-side compaction via `POST /v1/responses/compact`.

use serde_json::Value;

use super::{bodies::build_xai_compact_body, http::XaiFailedAttempt, XaiProvider};
use crate::model::{ModelError, ModelRequest, ModelUsage};
use crate::protocol::openai_responses::{retained_system_messages, CompactUserRetention};
use crate::providers::native_compaction::{
    native_compact_failure, native_compact_from_response_body,
};

/// Portable notice when the encrypted compaction artifact cannot replay.
///
/// xAI returns a single compaction item that stands in for the whole prior
/// conversation — there are no retained recent user messages below this notice.
pub(crate) const COMPACT_PORTABLE_HANDOFF_NOTICE: &str = "\
Context was compacted with xAI server-side compaction. Prior turns, including \
system prompts folded into the artifact, live in an encrypted blob that only \
compatible xAI Responses turns can read.";

impl XaiProvider {
    pub(super) async fn native_compact_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<rho_sdk::provider::NativeCompactionResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        let identity = self.model_identity();
        let retained_system_messages = retained_system_messages(request.messages);
        let body = match build_xai_compact_body(self.provider, &self.model, request) {
            Ok(body) => body,
            Err(error) => return Ok(native_compact_failure(error, Vec::new())),
        };

        let result = self
            .post_with_auth_retry("responses/compact", &body, Some(&cancellation), || Ok(()))
            .await;
        let failed_attempts = native_failed_attempts(result.failed_attempts);
        let response = match result.response {
            Ok(response) => response,
            Err(error) => return Ok(native_compact_failure(error, failed_attempts)),
        };
        if !response.status().is_success() {
            let error = cancel_aware_error_body(&cancellation, response).await;
            return Ok(native_compact_failure(error, failed_attempts));
        }

        let body = tokio::select! {
            result = response.json::<Value>() => match result {
                Ok(body) => body,
                Err(error) => {
                    return Ok(native_compact_failure(ModelError::from(error), failed_attempts));
                }
            },
            () = cancellation.cancelled() => {
                return Ok(native_compact_failure(ModelError::Interrupted, failed_attempts));
            }
        };

        Ok(native_compact_from_response_body(
            identity,
            &retained_system_messages,
            &body,
            COMPACT_PORTABLE_HANDOFF_NOTICE,
            CompactUserRetention::CompactionItemOnly,
            failed_attempts,
        ))
    }
}

fn native_failed_attempts(
    attempts: Vec<XaiFailedAttempt>,
) -> Vec<rho_sdk::provider::NativeCompactionFailedAttempt> {
    attempts
        .into_iter()
        .map(|attempt| {
            let kind = match attempt {
                XaiFailedAttempt::Authentication => rho_sdk::ProviderErrorKind::Authentication,
            };
            rho_sdk::provider::NativeCompactionFailedAttempt::new(kind, ModelUsage::default())
        })
        .collect()
}

async fn cancel_aware_error_body(
    cancellation: &rho_sdk::CancellationToken,
    response: reqwest::Response,
) -> ModelError {
    tokio::select! {
        error = crate::provider_backend::http_error::from_response(response) => error,
        () = cancellation.cancelled() => ModelError::Interrupted,
    }
}
