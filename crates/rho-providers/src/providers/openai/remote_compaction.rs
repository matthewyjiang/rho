//! OpenAI server-side compaction via `POST /responses/compact`.
//!
//! Both Codex and direct API-key OpenAI use the unary compact endpoint. The
//! server returns replacement output items (retained user messages plus one
//! encrypted compaction item). Subsequent compatible turns must use the
//! Responses API so the compaction item can be replayed.

use crate::model::ModelRequest;
use crate::protocol::openai_responses::{retained_system_messages, CompactUserRetention};
use crate::providers::native_compaction::{
    native_compact_failure, native_compact_from_http, CompactParsePolicy,
};
use crate::providers::responses_http::{ResponsesEndpoint, ResponsesHttpTransport};

use super::responses_post;

use super::auth::Auth;
use super::codex_request::{build_responses_compact_body, ResponsesProfile};
use super::codex_ws::CodexWsTransport;
use super::configuration_update::reasoning_effort_context;
use super::reasoning::OpenAiReasoningProfile;

/// Portable notice shown when the encrypted compaction artifact cannot replay
/// (model/provider/API switch). Server-returned user messages remain in history.
const PORTABLE_HANDOFF_NOTICE: &str = "\
Context was compacted with OpenAI server-side compaction. Prior assistant replies \
and tool results live in an encrypted artifact that only compatible OpenAI Responses \
turns can read. Retained recent user messages are kept below.";

/// Inputs for one OpenAI/Codex compact HTTP round-trip.
pub(super) struct CompactHttp<'a> {
    pub auth: Option<&'a Auth>,
    pub profile: &'a ResponsesProfile,
    pub reasoning_profile: &'a OpenAiReasoningProfile,
    pub http: &'a ResponsesHttpTransport<'a>,
    pub client: &'a reqwest::Client,
    pub refresh_url: &'a str,
    pub codex_ws: &'a CodexWsTransport,
}

fn compact_assistant_context(
    identity: &crate::model::ModelIdentity,
    body: &serde_json::Value,
) -> Vec<crate::model::ProviderContextBlock> {
    body.pointer("/reasoning/effort")
        .and_then(serde_json::Value::as_str)
        .and_then(|effort| reasoning_effort_context(identity, effort))
        .into_iter()
        .collect()
}

/// Runs native compaction through the shared Responses HTTP transport.
pub(super) async fn compact_with_http(
    compact: CompactHttp<'_>,
    request: ModelRequest<'_>,
) -> rho_sdk::provider::NativeCompactionResponse {
    let cancellation = request.cancellation.clone();
    let identity = compact.profile.identity().clone();
    // Only system messages are preserved from the source history; capture those
    // alone so the full conversation is not cloned across the HTTP round-trip.
    let retained_system_messages = retained_system_messages(request.messages);
    let body =
        match build_responses_compact_body(compact.profile, compact.reasoning_profile, request) {
            Ok(body) => body,
            Err(error) => return native_compact_failure(error, Vec::new()),
        };
    let assistant_context = compact_assistant_context(&identity, &body);

    let http_result = responses_post::post(
        compact.http,
        compact.client,
        compact.auth,
        compact.refresh_url,
        ResponsesEndpoint::Compact,
        &body,
        Some(&cancellation),
    )
    .await;
    let response = native_compact_from_http(
        http_result,
        &cancellation,
        CompactParsePolicy {
            identity,
            retained_system_messages: &retained_system_messages,
            portable_handoff_notice: PORTABLE_HANDOFF_NOTICE,
            user_retention: CompactUserRetention::KeepServerUsers,
            assistant_context: &assistant_context,
        },
    )
    .await;

    // History shape changed; drop any live previous_response_id baseline. A
    // failed compaction leaves history untouched, so the baseline stays valid.
    if matches!(compact.auth, Some(Auth::Codex { .. })) && response.result().is_ok() {
        compact.codex_ws.reset().await;
    }
    response
}

#[cfg(test)]
#[path = "remote_compaction_tests.rs"]
mod tests;
