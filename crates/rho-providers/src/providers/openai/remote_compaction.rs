//! OpenAI server-side compaction.
//!
//! Direct API-key OpenAI uses the unary `POST /responses/compact` endpoint,
//! which returns retained user messages plus one encrypted compaction item.
//! The Codex backend returns 404 for that endpoint, so Codex uses "remote
//! compaction v2": a streaming `POST /responses` whose input ends with a
//! `compaction_trigger` item. The server answers with only the compaction item,
//! so rho retains recent user messages itself. Subsequent compatible turns must
//! use the Responses API so the compaction item can be replayed.

use crate::model::ModelRequest;
use crate::protocol::openai_responses::{
    retained_system_and_recent_user_messages, retained_system_messages, CompactUserRetention,
    RETAINED_USER_MESSAGE_TOKEN_BUDGET,
};
use crate::providers::native_compaction::{
    native_compact_failure, native_compact_from_http, CompactBodyFormat, CompactParsePolicy,
};
use crate::providers::responses_http::{ResponsesEndpoint, ResponsesHttpTransport};

use super::responses_post;

use super::auth::Auth;
use super::codex_request::{
    build_compaction_trigger_body, build_responses_compact_body, NativeCompactionWire,
    ResponsesProfile,
};
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
    let wire = compact.profile.contract().native_compaction();
    // Capture only the messages the replacement keeps so the full conversation
    // is not cloned across the HTTP round-trip.
    let (retained_messages, endpoint, format, user_retention) = match wire {
        NativeCompactionWire::CompactEndpoint => (
            retained_system_messages(request.messages),
            ResponsesEndpoint::Compact,
            CompactBodyFormat::Json,
            CompactUserRetention::KeepServerUsers,
        ),
        NativeCompactionWire::CompactionTrigger => (
            retained_system_and_recent_user_messages(
                request.messages,
                RETAINED_USER_MESSAGE_TOKEN_BUDGET,
            ),
            ResponsesEndpoint::Create,
            CompactBodyFormat::ResponsesStream,
            CompactUserRetention::CompactionItemOnly,
        ),
    };
    let body = match wire {
        NativeCompactionWire::CompactEndpoint => {
            build_responses_compact_body(compact.profile, compact.reasoning_profile, request)
        }
        NativeCompactionWire::CompactionTrigger => {
            build_compaction_trigger_body(compact.profile, compact.reasoning_profile, request)
        }
    };
    let body = match body {
        Ok(body) => body,
        Err(error) => return native_compact_failure(error, Vec::new()),
    };
    let assistant_context = compact_assistant_context(&identity, &body);

    let http_result = responses_post::post(
        compact.http,
        compact.client,
        compact.auth,
        compact.refresh_url,
        endpoint,
        &body,
        Some(&cancellation),
    )
    .await;
    let response = native_compact_from_http(
        http_result,
        &cancellation,
        format,
        CompactParsePolicy {
            identity,
            retained_messages: &retained_messages,
            portable_handoff_notice: PORTABLE_HANDOFF_NOTICE,
            user_retention,
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
