//! xAI native server-side compaction via `POST /v1/responses/compact`.

use super::{bodies::build_xai_compact_body, XaiProvider};
use crate::model::{ModelError, ModelRequest};
use crate::protocol::openai_responses::{retained_system_messages, CompactUserRetention};
use crate::providers::native_compaction::{
    native_compact_failure, native_compact_from_http, CompactParsePolicy,
};
use crate::providers::responses_http::ResponsesEndpoint;

/// Portable notice when the encrypted compaction artifact cannot replay.
///
/// xAI returns a single compaction item that stands in for the whole prior
/// conversation — there are no retained recent user messages below this notice.
pub(crate) const COMPACT_PORTABLE_HANDOFF_NOTICE: &str = "\
Context was compacted with xAI server-side compaction. Prior turns, including \
system prompts folded into the artifact, live in an encrypted blob that only \
compatible xAI Responses turns can read.";

impl XaiProvider {
    /// Every xAI host this transport targets serves `/responses/compact`.
    pub(super) fn native_compact_available(&self) -> bool {
        true
    }

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

        let http_result = self
            .post_responses(
                ResponsesEndpoint::Compact,
                &body,
                Some(&cancellation),
                || Ok(()),
            )
            .await;
        Ok(native_compact_from_http(
            http_result,
            &cancellation,
            CompactParsePolicy {
                identity,
                retained_system_messages: &retained_system_messages,
                portable_handoff_notice: COMPACT_PORTABLE_HANDOFF_NOTICE,
                user_retention: CompactUserRetention::CompactionItemOnly,
            },
        )
        .await)
    }
}
