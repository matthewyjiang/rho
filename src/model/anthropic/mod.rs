mod convert;
mod stream;
mod types;

use crate::credentials::{load_provider_api_key, OsCredentialStore};
use crate::model::{
    models_dev::cached_model_metadata,
    provider_models::cached_provider_model,
    registry::{self, ProviderAuthKind},
    ModelError, ModelEvent, ModelProvider, ModelRequest, ModelResponse,
};

use convert::{convert_anthropic_response, split_system_and_messages, to_anthropic_tool};
use stream::collect_anthropic_sse_response;
use types::{AnthropicRequest, AnthropicResponse};

const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
    model: String,
    max_tokens: u32,
}

impl AnthropicProvider {
    pub fn new(model: String) -> Result<Self, ModelError> {
        let api_key = load_anthropic_api_key_auth()?;
        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            api_base: ANTHROPIC_API_BASE.into(),
            max_tokens: anthropic_max_tokens(&model),
            model,
        })
    }

    fn request_body(
        &self,
        request: ModelRequest,
        stream: bool,
    ) -> Result<AnthropicRequest, ModelError> {
        let (system, messages) = split_system_and_messages(request.messages)?;
        let tools = request
            .tools
            .into_iter()
            .map(to_anthropic_tool)
            .collect::<Vec<_>>();
        Ok(AnthropicRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system,
            messages,
            tools: (!tools.is_empty()).then_some(tools),
            stream,
        })
    }

    async fn send_messages(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let body = self.request_body(request, false)?;
        let response = self
            .client
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await?;
        let response = error_for_status_with_body(response).await?;
        let response: AnthropicResponse = response.json().await?;
        convert_anthropic_response(response)
    }

    async fn send_messages_stream(
        &self,
        request: ModelRequest,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        let body = self.request_body(request, true)?;
        let response = self
            .client
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await?;
        let response = error_for_status_with_body(response).await?;
        collect_anthropic_sse_response(response, on_event).await
    }

    fn messages_url(&self) -> String {
        format!("{}/messages", self.api_base.trim_end_matches('/'))
    }
}

fn anthropic_max_tokens(model: &str) -> u32 {
    cached_provider_model("anthropic", model)
        .and_then(|metadata| metadata.max_output_tokens)
        .or_else(|| {
            cached_model_metadata("anthropic", model)
                .and_then(|metadata| metadata.max_output_tokens)
        })
        .and_then(|tokens| u32::try_from(tokens).ok())
        .unwrap_or(DEFAULT_MAX_TOKENS)
}

fn load_anthropic_api_key_auth() -> Result<String, ModelError> {
    let descriptor = registry::provider_descriptor("anthropic")
        .ok_or_else(|| ModelError::UnsupportedProvider("anthropic".into()))?;
    let ProviderAuthKind::ApiKey {
        env_var, missing, ..
    } = descriptor.auth_kind
    else {
        return Err(ModelError::UnsupportedProvider("anthropic".into()));
    };
    if let Ok(key) = std::env::var(env_var) {
        return Ok(key);
    }
    let store = OsCredentialStore;
    load_provider_api_key(&store, descriptor.name)?
        .ok_or_else(|| registry::missing_credential_error(missing))
}

async fn error_for_status_with_body(
    response: reqwest::Response,
) -> Result<reqwest::Response, ModelError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(ModelError::HttpStatus { status, body })
}

#[async_trait::async_trait(?Send)]
impl ModelProvider for AnthropicProvider {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.send_messages(request).await
    }

    async fn send_turn_stream(
        &self,
        request: ModelRequest,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        self.send_messages_stream(request, on_event).await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::{ContentBlock, Message};
    use crate::tool::{ToolCall, ToolSpec};

    fn test_provider() -> AnthropicProvider {
        AnthropicProvider {
            client: reqwest::Client::new(),
            api_key: "test-key".into(),
            api_base: "https://example.test/v1".into(),
            model: "claude-sonnet-4-5".into(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    #[test]
    fn request_body_serializes_messages_tools_and_stream_flag() {
        let provider = test_provider();
        let body = provider
            .request_body(
                ModelRequest {
                    messages: vec![
                        Message::System("system prompt".into()),
                        Message::User(vec![ContentBlock::Text("hello".into())]),
                        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
                            id: "toolu_1".into(),
                            name: "bash".into(),
                            arguments: json!({"command":"pwd"}),
                        })]),
                    ],
                    tools: vec![ToolSpec {
                        name: "bash".into(),
                        description: "run command".into(),
                        input_schema: json!({"type":"object"}),
                    }],
                    prompt_cache_key: Some("ignored".into()),
                },
                true,
            )
            .unwrap();

        let value = serde_json::to_value(body).unwrap();
        assert_eq!(value["model"], "claude-sonnet-4-5");
        assert_eq!(value["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(value["system"], "system prompt");
        assert_eq!(value["stream"], true);
        assert_eq!(value["tools"][0]["name"], "bash");
        assert!(value.get("prompt_cache_key").is_none());
        assert_eq!(value["messages"][1]["content"][0]["type"], "tool_use");
    }
}
