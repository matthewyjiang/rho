use std::sync::Arc;

use crate::protocol::openai_responses::{
    codex_input_items_for_target, collect_codex_sse_response, to_responses_lite_tool,
};
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::{
    auth::xai_token::XaiAuthManager,
    credentials::CredentialStore,
    model::{ModelError, ModelEvent, ModelIdentity, ModelProvider, ModelRequest, ModelResponse},
    provider_backend::stream_timeout::provider_client,
    reasoning::ReasoningLevel,
};

const API_BASE: &str = "https://api.x.ai/v1";

pub struct XaiProvider {
    client: reqwest::Client,
    model: String,
    auth: XaiAuthManager,
    api_base: String,
    reasoning_effort: Option<String>,
}

impl XaiProvider {
    pub(crate) fn new(
        model: String,
        store: Arc<dyn CredentialStore>,
        reasoning: ReasoningLevel,
    ) -> Result<Self, ModelError> {
        Self::new_with_api_base(model, store, reasoning, API_BASE.into())
    }

    fn new_with_api_base(
        model: String,
        store: Arc<dyn CredentialStore>,
        reasoning: ReasoningLevel,
        api_base: String,
    ) -> Result<Self, ModelError> {
        let reasoning_effort = xai_reasoning_effort(&model, reasoning).map(str::to_string);
        Ok(Self {
            client: provider_client(),
            model,
            auth: XaiAuthManager::new(store)?,
            api_base,
            reasoning_effort,
        })
    }

    async fn send_request(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<reqwest::Response, ModelError> {
        let body =
            build_xai_responses_body(&self.model, request, self.reasoning_effort.as_deref())?;
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
    ) -> Result<ModelResponse, ModelError> {
        let response = self.send_request(request).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ModelError::HttpStatus { status, body });
        }
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
        let response = self.send_request(request).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ModelError::HttpStatus { status, body });
        }
        crate::providers::send_stream::collect_codex_model_response_silent(response).await
    }

    /// Streams one turn through a `Send` callback for the public SDK adapter.
    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = self.send_responses_turn(request, Some(on_event)) => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ModelProvider for XaiProvider {
    fn identity(&self) -> Option<ModelIdentity> {
        Some(self.model_identity())
    }

    fn set_reasoning(&mut self, reasoning: ReasoningLevel) -> bool {
        self.reasoning_effort = xai_reasoning_effort(&self.model, reasoning).map(str::to_string);
        true
    }

    async fn send_turn(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ModelError> {
        self.complete_turn(request).await
    }

    async fn send_turn_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = self.send_responses_turn(request, Some(on_event)) => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        }
    }
}

fn build_xai_responses_body(
    model: &str,
    request: ModelRequest<'_>,
    reasoning_effort: Option<&str>,
) -> Result<Value, ModelError> {
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

fn xai_reasoning_effort(model: &str, reasoning: ReasoningLevel) -> Option<&'static str> {
    match model {
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
    }
}

#[cfg(test)]
#[path = "xai_tests.rs"]
mod tests;
