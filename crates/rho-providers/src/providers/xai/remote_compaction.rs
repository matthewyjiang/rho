//! xAI server-side compaction via `POST /responses/compact`.
//!
//! Both API-key and OAuth xAI identities share this unary compact endpoint. The
//! response is an OpenAI-compatible compaction object; subsequent compatible
//! turns must stay on the xAI Responses API so the encrypted item can replay.

use reqwest::StatusCode;
use serde_json::Value;

use crate::{
    auth::xai_token::XaiAuthManager,
    model::{Message, ModelError, ModelIdentity, ModelRequest, ModelUsage},
    protocol::openai_responses::parse_compact_response,
};

use super::reasoning::XaiReasoningProfile;

/// Portable notice shown when the encrypted compaction artifact cannot replay
/// (model/provider/API switch). Host-owned system prompts remain in history.
const PORTABLE_HANDOFF_NOTICE: &str = "\
Context was compacted with xAI server-side compaction. Prior assistant replies \
and tool results live in an encrypted artifact that only compatible xAI Responses \
turns can read. Retained recent user messages are kept below.";

fn failure_response(
    error: ModelError,
    failed_attempts: Vec<rho_sdk::provider::NativeCompactionFailedAttempt>,
) -> rho_sdk::provider::NativeCompactionResponse {
    rho_sdk::provider::NativeCompactionResponse::failure(
        crate::providers::sdk_contract::provider_error_from_model_error(error),
    )
    .with_failed_attempts(failed_attempts)
}

/// Runs native compaction against xAI's Responses compact endpoint.
pub(super) async fn compact_with_http(
    client: &reqwest::Client,
    api_base: &str,
    auth: &XaiAuthManager,
    provider: &'static str,
    model: &str,
    reasoning: &XaiReasoningProfile,
    request: ModelRequest<'_>,
) -> rho_sdk::provider::NativeCompactionResponse {
    let cancellation = request.cancellation.clone();
    let identity = ModelIdentity::new(provider, "openai-responses", model);
    let retained_system_messages = request
        .messages
        .iter()
        .filter(|message| matches!(message, Message::System(_)))
        .cloned()
        .collect::<Vec<_>>();
    let body = match build_compact_request_body(provider, model, reasoning, request) {
        Ok(body) => body,
        Err(error) => return failure_response(error, Vec::new()),
    };

    let mut failed_attempts = Vec::new();
    let auth_material = match auth.auth_material().await {
        Ok(material) => material,
        Err(error) => return failure_response(error, failed_attempts),
    };
    let response = match post_compact(client, api_base, &auth_material.access_token, &body).await {
        Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {
            failed_attempts.push(rho_sdk::provider::NativeCompactionFailedAttempt::new(
                rho_sdk::ProviderErrorKind::Authentication,
                ModelUsage::default(),
            ));
            match auth.force_refresh(&auth_material.access_token).await {
                Ok(Some(refreshed)) => {
                    match post_compact(client, api_base, &refreshed.access_token, &body).await {
                        Ok(response) => response,
                        Err(error) => return failure_response(error, failed_attempts),
                    }
                }
                Ok(None) => response,
                Err(error) => return failure_response(error, failed_attempts),
            }
        }
        Ok(response) => response,
        Err(error) => return failure_response(error, failed_attempts),
    };

    if !response.status().is_success() {
        return failure_response(
            crate::provider_backend::http_error::from_response(response).await,
            failed_attempts,
        );
    }

    let body = tokio::select! {
        result = response.json::<Value>() => match result {
            Ok(body) => body,
            Err(error) => return failure_response(ModelError::from(error), failed_attempts),
        },
        () = cancellation.cancelled() => {
            return failure_response(ModelError::Interrupted, failed_attempts);
        }
    };

    let (messages, usage) = match parse_compact_response(
        identity,
        &retained_system_messages,
        &body,
        PORTABLE_HANDOFF_NOTICE,
    ) {
        Ok(parsed) => parsed,
        Err(error) => return failure_response(error, failed_attempts),
    };
    let output = match rho_sdk::CompactionOutput::with_usage(messages, usage) {
        Ok(output) => output,
        Err(error) => {
            return failure_response(
                ModelError::InvalidResponse(error.to_string()),
                failed_attempts,
            );
        }
    };
    rho_sdk::provider::NativeCompactionResponse::success(output)
        .with_failed_attempts(failed_attempts)
}

async fn post_compact(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    body: &Value,
) -> Result<reqwest::Response, ModelError> {
    Ok(client
        .post(format!(
            "{}/responses/compact",
            api_base.trim_end_matches('/')
        ))
        .bearer_auth(access_token)
        .header("User-Agent", crate::rho_user_agent())
        .json(body)
        .send()
        .await?)
}

/// Builds a unary `/responses/compact` body. Compact never streams or advertises tools.
pub(super) fn build_compact_request_body(
    provider: &'static str,
    model: &str,
    reasoning: &XaiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    super::build_xai_responses_body(
        provider,
        model,
        reasoning,
        request,
        super::XaiResponsesMode::Compact,
    )
}

#[cfg(test)]
#[path = "remote_compaction_tests.rs"]
mod tests;
