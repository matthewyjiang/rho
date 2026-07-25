#[cfg(test)]
use std::sync::Arc;

use crate::protocol::openai_responses::{
    codex_input_items_for_target, collect_codex_sse_response, native_compact_failure,
    native_compact_from_response_body, retained_system_messages, to_responses_lite_tool,
};
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::{
    auth::xai_token::XaiAuthManager,
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage},
};

#[cfg(test)]
use crate::{credentials::CredentialStore, provider_backend::stream_timeout::provider_client};

#[path = "xai/reasoning.rs"]
mod reasoning;

/// Portable notice shown when the encrypted compaction artifact cannot replay
/// (model/provider/API switch). Host-owned system prompts remain in history.
const COMPACT_PORTABLE_HANDOFF_NOTICE: &str = "\
Context was compacted with xAI server-side compaction. Prior assistant replies \
and tool results live in an encrypted artifact that only compatible xAI Responses \
turns can read. Retained recent user messages are kept below.";

pub struct XaiProvider {
    client: reqwest::Client,
    provider: &'static str,
    model: String,
    auth: XaiAuthManager,
    api_base: String,
    reasoning: reasoning::XaiReasoningProfile,
}

/// Outcome of an xAI JSON POST that may refresh once on `401`.
struct XaiHttpResult {
    response: Result<reqwest::Response, ModelError>,
    /// True when the first attempt returned `401` (refresh may or may not have run).
    auth_challenge: bool,
}

impl XaiProvider {
    pub(crate) fn new_with_transport(
        provider: &'static str,
        model: String,
        auth: XaiAuthManager,
        client: reqwest::Client,
        api_base: String,
    ) -> Self {
        let reasoning = reasoning::XaiReasoningProfile::from_metadata(
            &model,
            crate::model::models_dev::current_model_metadata(provider, &model),
        );
        Self {
            client,
            provider,
            model,
            auth,
            api_base,
            reasoning,
        }
    }

    #[cfg(test)]
    fn new_with_api_base(
        model: String,
        store: Arc<dyn CredentialStore>,
        api_base: String,
    ) -> Result<Self, ModelError> {
        Ok(Self::new_with_transport(
            "xai",
            model,
            XaiAuthManager::new(store)?,
            provider_client(),
            api_base,
        ))
    }

    async fn send_request(
        &self,
        request: ModelRequest<'_>,
        on_request_event: Option<
            &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                      + Send),
        >,
    ) -> Result<reqwest::Response, ModelError> {
        let body = build_xai_responses_body(self.provider, &self.model, &self.reasoning, request)?;
        let mut on_request_event = on_request_event;
        let result = self
            .post_with_auth_retry("responses", &body, || {
                if let Some(on_request_event) = on_request_event.as_mut() {
                    on_request_event(
                        rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                            kind: rho_sdk::ProviderErrorKind::Authentication,
                            usage: ModelUsage::default(),
                        },
                    )?;
                }
                Ok(())
            })
            .await;
        result.response
    }

    async fn post_json(
        &self,
        path: &str,
        access_token: &str,
        body: &Value,
    ) -> Result<reqwest::Response, ModelError> {
        Ok(self
            .client
            .post(format!(
                "{}/{}",
                self.api_base.trim_end_matches('/'),
                path.trim_start_matches('/')
            ))
            .bearer_auth(access_token)
            .header("User-Agent", crate::rho_user_agent())
            .json(body)
            .send()
            .await?)
    }

    /// Posts JSON and refreshes once on `401` when credentials allow it.
    ///
    /// `before_retry` runs only after a successful refresh and before the second
    /// POST (create uses this to emit a request-attempt event).
    async fn post_with_auth_retry(
        &self,
        path: &str,
        body: &Value,
        before_retry: impl FnOnce() -> Result<(), ModelError>,
    ) -> XaiHttpResult {
        let auth = match self.auth.auth_material().await {
            Ok(auth) => auth,
            Err(error) => {
                return XaiHttpResult {
                    response: Err(error),
                    auth_challenge: false,
                };
            }
        };
        let response = match self.post_json(path, &auth.access_token, body).await {
            Ok(response) => response,
            Err(error) => {
                return XaiHttpResult {
                    response: Err(error),
                    auth_challenge: false,
                };
            }
        };
        if response.status() != StatusCode::UNAUTHORIZED {
            return XaiHttpResult {
                response: Ok(response),
                auth_challenge: false,
            };
        }
        match self.auth.force_refresh(&auth.access_token).await {
            Ok(None) => XaiHttpResult {
                response: Ok(response),
                auth_challenge: true,
            },
            Ok(Some(refreshed)) => {
                if let Err(error) = before_retry() {
                    return XaiHttpResult {
                        response: Err(error),
                        auth_challenge: true,
                    };
                }
                XaiHttpResult {
                    response: self.post_json(path, &refreshed.access_token, body).await,
                    auth_challenge: true,
                }
            }
            Err(error) => XaiHttpResult {
                response: Err(error),
                auth_challenge: true,
            },
        }
    }

    async fn send_responses_turn(
        &self,
        request: ModelRequest<'_>,
        mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
        on_request_event: Option<
            &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                      + Send),
        >,
    ) -> Result<ModelResponse, ModelError> {
        let response = self.send_request(request, on_request_event).await?;
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
        collect_codex_sse_response(response, &mut on_event)
            .await
            .map(|output| output.response)
    }
}

impl XaiProvider {
    pub(crate) fn model_identity(&self) -> ModelIdentity {
        ModelIdentity::new(self.provider, "openai-responses", &self.model)
    }

    /// Completes one turn using a `Send` future suitable for the public SDK trait.
    pub(crate) async fn complete_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let response = self.send_request(request, None).await?;
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
        crate::providers::send_stream::collect_codex_model_response_silent(response).await
    }

    /// Streams one turn through a `Send` callback for the public SDK adapter.
    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = self.send_responses_turn(request, Some(on_event), Some(on_request_event)) => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        }
    }

    async fn native_compact_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<rho_sdk::provider::NativeCompactionResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        let identity = self.model_identity();
        let retained_system_messages = retained_system_messages(request.messages);
        let body =
            match build_xai_compact_body(self.provider, &self.model, &self.reasoning, request) {
                Ok(body) => body,
                Err(error) => return Ok(native_compact_failure(error, Vec::new())),
            };

        let result = self
            .post_with_auth_retry("responses/compact", &body, || Ok(()))
            .await;
        let mut failed_attempts = Vec::new();
        if result.auth_challenge {
            failed_attempts.push(rho_sdk::provider::NativeCompactionFailedAttempt::new(
                rho_sdk::ProviderErrorKind::Authentication,
                ModelUsage::default(),
            ));
        }
        let response = match result.response {
            Ok(response) => response,
            Err(error) => return Ok(native_compact_failure(error, failed_attempts)),
        };
        if !response.status().is_success() {
            return Ok(native_compact_failure(
                crate::provider_backend::http_error::from_response(response).await,
                failed_attempts,
            ));
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
            failed_attempts,
        ))
    }
}

crate::impl_sdk_model_provider!(XaiProvider, native_compact);

/// Shared lowered fields for xAI Responses create and compact bodies.
struct XaiResponsesLowered {
    instructions: String,
    input: Vec<Value>,
    prompt_cache_key: Option<String>,
    reasoning_effort: Option<&'static str>,
}

fn lower_xai_responses_request(
    provider: &'static str,
    model: &str,
    reasoning: &reasoning::XaiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<XaiResponsesLowered, ModelError> {
    let mut instructions = Vec::new();
    let target = ModelIdentity::new(provider, "openai-responses", model);
    let input =
        codex_input_items_for_target(request.messages.to_vec(), &mut instructions, Some(&target))?;
    Ok(XaiResponsesLowered {
        instructions: instructions.join("\n\n"),
        input,
        prompt_cache_key: request.prompt_cache_key.map(str::to_owned),
        reasoning_effort: reasoning.effort(request.reasoning_level),
    })
}

fn attach_xai_prompt_cache_and_reasoning(
    body: &mut Value,
    instructions: String,
    prompt_cache_key: Option<String>,
    reasoning_effort: Option<&'static str>,
) {
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions);
    }
    if let Some(prompt_cache_key) = prompt_cache_key {
        body["prompt_cache_key"] = json!(prompt_cache_key);
    }
    if let Some(effort) = reasoning_effort {
        body["reasoning"] = json!({ "effort": effort });
    }
}

/// Builds a streaming Responses create body for an xAI model turn.
///
/// Always requests encrypted reasoning content so later server-side compaction
/// can fold prior thinking into the opaque artifact.
fn build_xai_responses_body(
    provider: &'static str,
    model: &str,
    reasoning: &reasoning::XaiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    let tools = request
        .tools
        .iter()
        .cloned()
        .map(to_responses_lite_tool)
        .collect::<Vec<_>>();
    let XaiResponsesLowered {
        instructions,
        input,
        prompt_cache_key,
        reasoning_effort,
    } = lower_xai_responses_request(provider, model, reasoning, request)?;
    let mut body = json!({
        "model": model,
        "input": input,
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content"],
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
    }
    attach_xai_prompt_cache_and_reasoning(
        &mut body,
        instructions,
        prompt_cache_key,
        reasoning_effort,
    );
    Ok(body)
}

/// Builds a unary `/responses/compact` body.
///
/// Compact never streams, never advertises tools, and never requests extra
/// include fields.
fn build_xai_compact_body(
    provider: &'static str,
    model: &str,
    reasoning: &reasoning::XaiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    let XaiResponsesLowered {
        instructions,
        input,
        prompt_cache_key,
        reasoning_effort,
    } = lower_xai_responses_request(provider, model, reasoning, request)?;
    let mut body = json!({
        "model": model,
        "input": input,
        "store": false,
    });
    attach_xai_prompt_cache_and_reasoning(
        &mut body,
        instructions,
        prompt_cache_key,
        reasoning_effort,
    );
    Ok(body)
}

#[cfg(test)]
#[path = "xai_tests.rs"]
mod tests;
