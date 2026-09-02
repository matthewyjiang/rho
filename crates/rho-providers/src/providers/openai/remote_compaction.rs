//! OpenAI server-side compaction via `POST /responses/compact`.
//!
//! Both Codex and direct API-key OpenAI use the unary compact endpoint. The
//! server returns replacement output items (retained user messages plus one
//! encrypted compaction item). Subsequent compatible turns must use the
//! Responses API so the compaction item can be replayed.

use crate::model::ModelRequest;
use crate::protocol::openai_responses::{retained_system_messages, CompactUserRetention};
use crate::providers::native_compaction::{
    compact_over_responses_http, native_compact_failure, CompactParsePolicy,
};
use crate::providers::responses_http::{ResponsesAuth, ResponsesHttpTransport};

use super::auth::Auth;
use super::codex_request::{build_responses_compact_body, ResponsesProfile};
use super::codex_ws::CodexWsTransport;
use super::reasoning::OpenAiReasoningProfile;

/// Portable notice shown when the encrypted compaction artifact cannot replay
/// (model/provider/API switch). Server-returned user messages remain in history.
const PORTABLE_HANDOFF_NOTICE: &str = "\
Context was compacted with OpenAI server-side compaction. Prior assistant replies \
and tool results live in an encrypted artifact that only compatible OpenAI Responses \
turns can read. Retained recent user messages are kept below.";

/// Runs native compaction through the shared Responses HTTP transport.
pub(super) async fn compact_with_http(
    auth: Option<&Auth>,
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
    let body = match build_responses_compact_body(profile, reasoning_profile, request) {
        Ok(body) => body,
        Err(error) => return native_compact_failure(error, Vec::new()),
    };

    let response = compact_over_responses_http(
        http,
        ResponsesAuth::from_openai(auth),
        &body,
        &cancellation,
        CompactParsePolicy {
            identity,
            retained_system_messages: &retained_system_messages,
            portable_handoff_notice: PORTABLE_HANDOFF_NOTICE,
            user_retention: CompactUserRetention::KeepServerUsers,
        },
    )
    .await;

    // History shape changed; drop any live previous_response_id baseline. A
    // failed compaction leaves history untouched, so the baseline stays valid.
    if matches!(auth, Some(Auth::Codex { .. })) && response.result().is_ok() {
        codex_ws.reset().await;
    }
    response
}

#[cfg(test)]
#[path = "remote_compaction_tests.rs"]
mod tests;
