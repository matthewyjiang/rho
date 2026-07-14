use crate::{
    model::ModelIdentity,
    protocol::anthropic_messages::{
        collect_anthropic_sse_response, convert_anthropic_response, split_system_and_messages,
        to_anthropic_tool, AnthropicCacheControl, AnthropicContentBlock, AnthropicMessage,
        AnthropicOutputConfig, AnthropicRequest, AnthropicResponse, AnthropicRole,
        AnthropicSystemBlock, AnthropicThinkingConfig, ProviderContextReplay,
    },
    provider_backend::{
        stream_timeout::provider_client, ModelError, ModelEvent, ModelProvider, ModelRequest,
        ModelResponse,
    },
    reasoning::ReasoningLevel,
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
    reasoning: ReasoningLevel,
}

impl AnthropicProvider {
    pub fn new(model: String, api_key: String, max_tokens: fn(&str) -> u32) -> Self {
        let reasoning = default_reasoning(&model);
        Self {
            client: provider_client(),
            api_key,
            api_base: ANTHROPIC_API_BASE.into(),
            model,
            max_tokens,
            reasoning,
        }
    }

    fn request_body(
        &self,
        request: ModelRequest<'_>,
        stream: bool,
    ) -> Result<AnthropicRequest, ModelError> {
        let target = self.identity().expect("Anthropic provider has an identity");
        let max_tokens = (self.max_tokens)(&self.model);
        let (thinking, output_config) = thinking_config(&self.model, self.reasoning, max_tokens);
        let (system, mut messages) = split_system_and_messages(
            request.messages.to_vec(),
            &target,
            provider_context_replay(thinking.as_ref()),
        )?;
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
            output_config,
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

fn provider_context_replay(thinking: Option<&AnthropicThinkingConfig>) -> ProviderContextReplay {
    match thinking {
        Some(
            AnthropicThinkingConfig::Enabled { .. } | AnthropicThinkingConfig::Adaptive { .. },
        ) => ProviderContextReplay::Enabled,
        Some(AnthropicThinkingConfig::Disabled) | None => ProviderContextReplay::Disabled,
    }
}

fn thinking_config(
    model: &str,
    reasoning: ReasoningLevel,
    max_tokens: u32,
) -> (
    Option<AnthropicThinkingConfig>,
    Option<AnthropicOutputConfig>,
) {
    if reasoning == ReasoningLevel::Off {
        let thinking =
            supports_disabled_thinking(model).then_some(AnthropicThinkingConfig::Disabled);
        return (thinking, None);
    }
    if supports_adaptive_thinking(model) {
        return (
            Some(AnthropicThinkingConfig::Adaptive {
                display: "summarized",
            }),
            Some(AnthropicOutputConfig {
                effort: adaptive_effort(model, reasoning),
            }),
        );
    }

    let requested_budget = match reasoning {
        ReasoningLevel::Off => return (None, None),
        ReasoningLevel::Minimal => 1_024,
        ReasoningLevel::Low => 2_048,
        ReasoningLevel::Medium => 4_096,
        ReasoningLevel::High => 8_192,
        ReasoningLevel::Xhigh => 16_384,
        ReasoningLevel::Max => 32_768,
    };
    let available = max_tokens.saturating_sub(ANTHROPIC_ANSWER_RESERVE_TOKENS);
    let thinking = (available >= 1_024).then_some(AnthropicThinkingConfig::Enabled {
        budget_tokens: requested_budget.min(available),
    });
    (thinking, None)
}

fn supports_adaptive_thinking(model: &str) -> bool {
    const MODELS: &[&str] = &[
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
        "claude-sonnet-5",
        "claude-fable-5",
        "claude-mythos-5",
        "claude-mythos-preview",
    ];
    MODELS.iter().any(|prefix| model_matches(model, prefix))
}

fn default_reasoning(model: &str) -> ReasoningLevel {
    if adaptive_thinking_is_mandatory(model) {
        ReasoningLevel::Low
    } else {
        ReasoningLevel::Off
    }
}

fn adaptive_thinking_is_mandatory(model: &str) -> bool {
    const MODELS: &[&str] = &["claude-fable-5", "claude-mythos-5", "claude-mythos-preview"];
    MODELS.iter().any(|prefix| model_matches(model, prefix))
}

fn supports_disabled_thinking(model: &str) -> bool {
    model_matches(model, "claude-sonnet-5")
}

fn adaptive_effort(model: &str, reasoning: ReasoningLevel) -> &'static str {
    match reasoning {
        ReasoningLevel::Off | ReasoningLevel::Minimal | ReasoningLevel::Low => "low",
        ReasoningLevel::Medium => "medium",
        ReasoningLevel::High => "high",
        ReasoningLevel::Xhigh if supports_xhigh_effort(model) => "xhigh",
        ReasoningLevel::Xhigh => "high",
        ReasoningLevel::Max => "max",
    }
}

fn supports_xhigh_effort(model: &str) -> bool {
    const MODELS: &[&str] = &[
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-5",
        "claude-fable-5",
        "claude-mythos-5",
    ];
    MODELS.iter().any(|prefix| model_matches(model, prefix))
}

fn model_matches(model: &str, prefix: &str) -> bool {
    model == prefix
        || model
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('-'))
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

    fn set_reasoning(&mut self, reasoning: ReasoningLevel) -> bool {
        if reasoning == ReasoningLevel::Off && adaptive_thinking_is_mandatory(&self.model) {
            return false;
        }
        self.reasoning = reasoning;
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
#[path = "provider_tests.rs"]
mod tests;
