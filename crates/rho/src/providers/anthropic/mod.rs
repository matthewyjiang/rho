use crate::{
    model::ModelIdentity,
    protocol::anthropic_messages::{
        collect_anthropic_sse_response, convert_anthropic_response, split_system_and_messages,
        to_anthropic_tool, AnthropicCacheControl, AnthropicContentBlock, AnthropicMessage,
        AnthropicOutputConfig, AnthropicRequest, AnthropicResponse, AnthropicRole,
        AnthropicSystemBlock, AnthropicThinkingConfig, ProviderContextReplay,
    },
    provider_backend::{ModelError, ModelEvent, ModelRequest, ModelResponse},
    reasoning::ReasoningLevel,
};

#[cfg(test)]
use crate::provider_backend::stream_timeout::provider_client;

#[cfg(test)]
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
}

impl AnthropicProvider {
    #[cfg(test)]
    pub fn new(model: String, api_key: String, max_tokens: fn(&str) -> u32) -> Self {
        Self::new_with_transport(
            model,
            api_key,
            max_tokens,
            provider_client(),
            ANTHROPIC_API_BASE.into(),
        )
    }

    pub(crate) fn new_with_transport(
        model: String,
        api_key: String,
        max_tokens: fn(&str) -> u32,
        client: reqwest::Client,
        api_base: String,
    ) -> Self {
        Self {
            client,
            api_key,
            api_base,
            model,
            max_tokens,
        }
    }

    fn request_body(
        &self,
        request: ModelRequest<'_>,
        stream: bool,
    ) -> Result<AnthropicRequest, ModelError> {
        let target = self.model_identity();
        let max_tokens = (self.max_tokens)(&self.model);
        let (thinking, output_config) =
            thinking_config(&self.model, request.reasoning_level, max_tokens)?;
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

    pub(crate) fn model_identity(&self) -> ModelIdentity {
        ModelIdentity::new("anthropic", "anthropic-messages", &self.model)
    }

    /// Completes one turn using inherent async methods so the future is `Send`.
    pub(crate) async fn complete_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        self.send_messages(request).await
    }

    /// Streams one turn through a `Send` callback for the public SDK adapter.
    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = self.send_messages_stream(request, on_event) => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        }
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
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
        let response: AnthropicResponse = response.json().await?;
        convert_anthropic_response(response)
    }

    async fn send_messages_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
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
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
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
) -> Result<
    (
        Option<AnthropicThinkingConfig>,
        Option<AnthropicOutputConfig>,
    ),
    ModelError,
> {
    if reasoning == ReasoningLevel::Off && adaptive_thinking_is_mandatory(model) {
        return Err(ModelError::UnsupportedReasoning {
            provider: "anthropic",
            model: model.to_string(),
            requested: reasoning,
        });
    }
    if reasoning == ReasoningLevel::Off {
        let thinking =
            supports_disabled_thinking(model).then_some(AnthropicThinkingConfig::Disabled);
        return Ok((thinking, None));
    }
    if supports_adaptive_thinking(model) {
        return Ok((
            Some(AnthropicThinkingConfig::Adaptive {
                display: "summarized",
            }),
            Some(AnthropicOutputConfig {
                effort: adaptive_effort(model, reasoning),
            }),
        ));
    }

    let requested_budget = match reasoning {
        ReasoningLevel::Off => return Ok((None, None)),
        ReasoningLevel::Minimal => 1_024,
        ReasoningLevel::Low => 2_048,
        ReasoningLevel::Medium => 4_096,
        ReasoningLevel::High => 8_192,
        ReasoningLevel::Xhigh => 16_384,
        ReasoningLevel::Max => 32_768,
    };
    let available = max_tokens.saturating_sub(ANTHROPIC_ANSWER_RESERVE_TOKENS);
    if available < 1_024 {
        return Err(ModelError::InvalidResponse(format!(
            "Anthropic max output tokens {max_tokens} cannot reserve a reasoning budget"
        )));
    }
    Ok((
        Some(AnthropicThinkingConfig::Enabled {
            budget_tokens: requested_budget.min(available),
        }),
        None,
    ))
}

#[derive(Clone, Copy, Default)]
struct ModelCapabilities {
    adaptive_thinking: bool,
    mandatory_adaptive_thinking: bool,
    disabled_thinking: bool,
    xhigh_effort: bool,
}

const fn capabilities(
    adaptive_thinking: bool,
    mandatory_adaptive_thinking: bool,
    disabled_thinking: bool,
    xhigh_effort: bool,
) -> ModelCapabilities {
    ModelCapabilities {
        adaptive_thinking,
        mandatory_adaptive_thinking,
        disabled_thinking,
        xhigh_effort,
    }
}

fn model_capabilities(model: &str) -> ModelCapabilities {
    const TABLE: &[(&str, ModelCapabilities)] = &[
        ("claude-opus-4-6", capabilities(true, false, false, false)),
        ("claude-opus-4-7", capabilities(true, false, false, true)),
        ("claude-opus-4-8", capabilities(true, false, false, true)),
        ("claude-sonnet-4-6", capabilities(true, false, false, false)),
        ("claude-sonnet-5", capabilities(true, false, true, true)),
        ("claude-fable-5", capabilities(true, true, false, true)),
        ("claude-mythos-5", capabilities(true, true, false, true)),
        (
            "claude-mythos-preview",
            capabilities(true, true, false, false),
        ),
    ];
    TABLE
        .iter()
        .find(|(prefix, _)| model_matches(model, prefix))
        .map(|(_, caps)| *caps)
        .unwrap_or_default()
}

fn supports_adaptive_thinking(model: &str) -> bool {
    model_capabilities(model).adaptive_thinking
}

fn adaptive_thinking_is_mandatory(model: &str) -> bool {
    model_capabilities(model).mandatory_adaptive_thinking
}

fn supports_disabled_thinking(model: &str) -> bool {
    model_capabilities(model).disabled_thinking
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
    model_capabilities(model).xhigh_effort
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

crate::impl_sdk_model_provider!(AnthropicProvider);

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
