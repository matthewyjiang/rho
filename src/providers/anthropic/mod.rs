use crate::{
    model::ModelIdentity,
    protocol::anthropic_messages::{
        collect_anthropic_sse_response, convert_anthropic_response, split_system_and_messages,
        to_anthropic_tool, AnthropicCacheControl, AnthropicContentBlock, AnthropicMessage,
        AnthropicRequest, AnthropicResponse, AnthropicRole, AnthropicSystemBlock,
        AnthropicThinkingConfig,
    },
    provider_backend::{
        stream_timeout::provider_client, ModelError, ModelEvent, ModelProvider, ModelRequest,
        ModelResponse,
    },
};

const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
const ANTHROPIC_ANSWER_RESERVE_TOKENS: u32 = 1_024;

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
    model: String,
    max_tokens: fn(&str) -> u32,
    thinking_budget_tokens: Option<u32>,
}

impl AnthropicProvider {
    pub fn new(model: String, api_key: String, max_tokens: fn(&str) -> u32) -> Self {
        Self {
            client: provider_client(),
            api_key,
            api_base: ANTHROPIC_API_BASE.into(),
            model,
            max_tokens,
            thinking_budget_tokens: None,
        }
    }

    fn request_body(
        &self,
        request: ModelRequest<'_>,
        stream: bool,
    ) -> Result<AnthropicRequest, ModelError> {
        let target = self.identity().expect("Anthropic provider has an identity");
        let (system, mut messages) = split_system_and_messages(request.messages.to_vec(), &target)?;
        mark_cache_control_points(&mut messages);
        let mut tools = request
            .tools
            .iter()
            .cloned()
            .map(to_anthropic_tool)
            .collect::<Vec<_>>();
        if let Some(tool) = tools.last_mut() {
            tool.cache_control = Some(AnthropicCacheControl::ephemeral());
        }
        let max_tokens = (self.max_tokens)(&self.model);
        let thinking = self.thinking_budget_tokens.and_then(|budget_tokens| {
            let available = max_tokens.saturating_sub(ANTHROPIC_ANSWER_RESERVE_TOKENS);
            (available >= 1_024).then_some(AnthropicThinkingConfig {
                kind: "enabled",
                budget_tokens: budget_tokens.min(available),
            })
        });
        Ok(AnthropicRequest {
            model: self.model.clone(),
            max_tokens,
            system: system.map(|text| {
                vec![AnthropicSystemBlock::text(
                    text,
                    Some(AnthropicCacheControl::ephemeral()),
                )]
            }),
            messages,
            tools: (!tools.is_empty()).then_some(tools),
            cache_control: None,
            thinking,
            stream,
        })
    }

    async fn send_messages(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ModelError> {
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
        request: ModelRequest<'_>,
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

fn mark_cache_control_points(messages: &mut [AnthropicMessage]) {
    let marker = AnthropicCacheControl::ephemeral();
    for message in messages.iter_mut().rev() {
        if message.role == AnthropicRole::User {
            let Some(block) = message.content.last_mut() else {
                return;
            };
            if let AnthropicContentBlock::Text { cache_control, .. }
            | AnthropicContentBlock::ToolResult { cache_control, .. } = block
            {
                *cache_control = Some(marker);
                return;
            }
        }
    }

    for message in messages.iter_mut().rev() {
        if message.role != AnthropicRole::Assistant {
            continue;
        }
        if let Some(AnthropicContentBlock::Text { cache_control, .. }) = message
            .content
            .iter_mut()
            .rev()
            .find(|block| matches!(block, AnthropicContentBlock::Text { .. }))
        {
            *cache_control = Some(marker);
            return;
        }
    }
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
    fn identity(&self) -> Option<ModelIdentity> {
        Some(ModelIdentity::new(
            "anthropic",
            "anthropic-messages",
            &self.model,
        ))
    }

    fn set_reasoning(&mut self, reasoning: crate::reasoning::ReasoningLevel) -> bool {
        self.thinking_budget_tokens = match reasoning {
            crate::reasoning::ReasoningLevel::Off => None,
            crate::reasoning::ReasoningLevel::Minimal => Some(1_024),
            crate::reasoning::ReasoningLevel::Low => Some(2_048),
            crate::reasoning::ReasoningLevel::Medium => Some(4_096),
            crate::reasoning::ReasoningLevel::High => Some(8_192),
            crate::reasoning::ReasoningLevel::Xhigh => Some(16_384),
            crate::reasoning::ReasoningLevel::Max => Some(32_768),
        };
        true
    }

    async fn send_turn(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ModelError> {
        self.send_messages(request).await
    }

    async fn send_turn_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = self.send_messages_stream(request, on_event) => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::provider_backend::{ContentBlock, Message, ToolCall, ToolSpec};

    fn test_provider() -> AnthropicProvider {
        AnthropicProvider {
            client: provider_client(),
            api_key: "test-key".into(),
            api_base: "https://example.test/v1".into(),
            model: "claude-sonnet-4-5".into(),
            max_tokens: |_| DEFAULT_MAX_TOKENS,
            thinking_budget_tokens: None,
        }
    }

    #[test]
    fn request_body_serializes_messages_tools_and_stream_flag() {
        let provider = test_provider();
        let body = provider
            .request_body(
                ModelRequest {
                    messages: &[
                        Message::System("system prompt".into()),
                        Message::User(vec![ContentBlock::Text("hello".into())]),
                        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
                            id: "toolu_1".into(),
                            name: "bash".into(),
                            arguments: json!({"command":"pwd"}),
                        })]),
                    ],
                    tools: &[ToolSpec {
                        name: "bash".into(),
                        description: "run command".into(),
                        input_schema: json!({"type":"object"}),
                    }],
                    cancellation: Default::default(),
                    prompt_cache_key: Some("ignored"),
                },
                true,
            )
            .unwrap();

        let value = serde_json::to_value(body).unwrap();
        assert_eq!(value["model"], "claude-sonnet-4-5");
        assert_eq!(value["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(value["system"][0]["text"], "system prompt");
        assert_eq!(
            value["system"][0]["cache_control"],
            json!({"type":"ephemeral"})
        );
        assert_eq!(value["stream"], true);
        assert_eq!(value["tools"][0]["name"], "bash");
        assert_eq!(
            value["tools"][0]["cache_control"],
            json!({"type":"ephemeral"})
        );
        assert!(value.get("cache_control").is_none());
        assert!(value.get("prompt_cache_key").is_none());
        assert_eq!(value["messages"][1]["content"][0]["type"], "tool_use");
    }

    #[test]
    fn thinking_budget_reserves_answer_tokens() {
        let mut provider = test_provider();
        provider.thinking_budget_tokens = Some(4_096);

        let body = provider
            .request_body(
                ModelRequest {
                    messages: &[Message::user_text("hello")],
                    tools: &[],
                    cancellation: Default::default(),
                    prompt_cache_key: None,
                },
                false,
            )
            .unwrap();

        assert_eq!(body.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(
            body.thinking,
            Some(AnthropicThinkingConfig {
                kind: "enabled",
                budget_tokens: DEFAULT_MAX_TOKENS - ANTHROPIC_ANSWER_RESERVE_TOKENS,
            })
        );
    }

    #[test]
    fn request_body_removes_top_level_schema_composition_from_tools() {
        let provider = test_provider();
        let body = provider
            .request_body(
                ModelRequest {
                    messages: &[Message::user_text("hello")],
                    tools: &[ToolSpec {
                        name: "edit_file".into(),
                        description: "edit files".into(),
                        input_schema: json!({
                            "type": "object",
                            "properties": {
                                "path": {"type": "string"},
                                "value": {
                                    "anyOf": [
                                        {"type": "string"},
                                        {"type": "null"}
                                    ]
                                }
                            },
                            "anyOf": [
                                {"required": ["path"]},
                                {"required": ["value"]}
                            ],
                            "oneOf": [{"type": "object"}],
                            "allOf": [{"type": "object"}]
                        }),
                    }],
                    cancellation: Default::default(),
                    prompt_cache_key: None,
                },
                false,
            )
            .unwrap();

        let value = serde_json::to_value(body).unwrap();
        let schema = &value["tools"][0]["input_schema"];
        assert!(schema.get("anyOf").is_none());
        assert!(schema.get("oneOf").is_none());
        assert!(schema.get("allOf").is_none());
        assert!(schema["properties"]["value"].get("anyOf").is_some());
        assert_eq!(schema["properties"]["path"]["type"], "string");
    }
}
