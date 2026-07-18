#[cfg(test)]
use std::sync::Arc;

use crate::protocol::openai_responses::{
    codex_input_items_for_target, collect_codex_sse_response, to_responses_lite_tool,
};
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::{
    auth::xai_token::XaiAuthManager,
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage},
    reasoning::ReasoningLevel,
};

#[cfg(test)]
use crate::{credentials::CredentialStore, provider_backend::stream_timeout::provider_client};

pub struct XaiProvider {
    client: reqwest::Client,
    model: String,
    auth: XaiAuthManager,
    api_base: String,
}

impl XaiProvider {
    pub(crate) fn new_with_transport(
        model: String,
        auth: XaiAuthManager,
        client: reqwest::Client,
        api_base: String,
    ) -> Self {
        Self {
            client,
            model,
            auth,
            api_base,
        }
    }

    #[cfg(test)]
    fn new_with_api_base(
        model: String,
        store: Arc<dyn CredentialStore>,
        api_base: String,
    ) -> Result<Self, ModelError> {
        Ok(Self::new_with_transport(
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
        let body = build_xai_responses_body(&self.model, request)?;
        let auth = self.auth.auth_material().await?;
        let response = self
            .send_request_with_token(&body, &auth.access_token)
            .await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let Some(refreshed) = self.auth.force_refresh(&auth.access_token).await? else {
            return Ok(response);
        };
        if let Some(on_request_event) = on_request_event {
            on_request_event(
                rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                    kind: rho_sdk::ProviderErrorKind::Authentication,
                    usage: ModelUsage::default(),
                },
            )?;
        }
        self.send_request_with_token(&body, &refreshed.access_token)
            .await
    }

    async fn send_request_with_token(
        &self,
        body: &Value,
        access_token: &str,
    ) -> Result<reqwest::Response, ModelError> {
        Ok(self
            .client
            .post(format!("{}/responses", self.api_base.trim_end_matches('/')))
            .bearer_auth(access_token)
            .header("User-Agent", concat!("rho/", env!("CARGO_PKG_VERSION")))
            .json(body)
            .send()
            .await?)
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
        ModelIdentity::new("xai", "openai-responses", &self.model)
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
}

crate::impl_sdk_model_provider!(XaiProvider);

fn build_xai_responses_body(model: &str, request: ModelRequest<'_>) -> Result<Value, ModelError> {
    let reasoning_effort = xai_reasoning_effort(model, request.reasoning_level)?;
    let mut instructions = Vec::new();
    let target = crate::model::ModelIdentity::new("xai", "openai-responses", model);
    let input =
        codex_input_items_for_target(request.messages.to_vec(), &mut instructions, Some(&target))?;
    let tools = request
        .tools
        .iter()
        .cloned()
        .map(to_responses_lite_tool)
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": model,
        "input": input,
        "store": false,
        "stream": true,
    });
    let instructions = instructions.join("\n\n");
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
    }
    if let Some(prompt_cache_key) = request.prompt_cache_key {
        body["prompt_cache_key"] = json!(prompt_cache_key);
    }
    if let Some(effort) = reasoning_effort {
        body["reasoning"] = json!({ "effort": effort });
    }
    Ok(body)
}

fn xai_reasoning_effort(
    model: &str,
    reasoning: ReasoningLevel,
) -> Result<Option<&'static str>, ModelError> {
    // Non-reasoning models omit the field regardless of the session default so
    // configured models like grok-build keep working without forcing reasoning=off.
    Ok(match model {
        "grok-4.5" => match reasoning {
            ReasoningLevel::Off | ReasoningLevel::Minimal | ReasoningLevel::Low => Some("low"),
            ReasoningLevel::Medium => Some("medium"),
            ReasoningLevel::High | ReasoningLevel::Xhigh | ReasoningLevel::Max => Some("high"),
        },
        "grok-4.3" => match reasoning {
            ReasoningLevel::Off => Some("none"),
            ReasoningLevel::Minimal | ReasoningLevel::Low => Some("low"),
            ReasoningLevel::Medium => Some("medium"),
            ReasoningLevel::High | ReasoningLevel::Xhigh | ReasoningLevel::Max => Some("high"),
        },
        "grok-build-0.1" | "grok-composer-2.5-fast" => None,
        _ => None,
    })
}

#[cfg(test)]
#[path = "xai_tests.rs"]
mod tests;
