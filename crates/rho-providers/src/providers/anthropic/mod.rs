use crate::{
    model::ModelIdentity,
    protocol::anthropic_messages::{
        collect_anthropic_sse_response, convert_anthropic_response, split_system_and_messages,
        to_anthropic_tool, AnthropicCacheControl, AnthropicContentBlock, AnthropicMessage,
        AnthropicRequest, AnthropicResponse, AnthropicRole, AnthropicSystemBlock,
        AnthropicThinkingConfig, ProviderContextReplay,
    },
    provider_backend::{ModelError, ModelEvent, ModelRequest, ModelResponse},
};

#[cfg(test)]
use crate::provider_backend::stream_timeout::provider_client;

#[cfg(test)]
const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
pub(crate) const ANTHROPIC_ANSWER_RESERVE_TOKENS: u32 = 1_024;

mod thinking;

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
    model: String,
    max_tokens: fn(&str) -> u32,
    thinking_protocol: thinking::AnthropicThinkingProtocol,
}

impl AnthropicProvider {
    // Tests construct with an empty protocol and inject one explicitly via
    // `with_thinking_protocol`, so they never touch the on-disk model cache.
    #[cfg(test)]
    pub fn new(model: String, api_key: String, max_tokens: fn(&str) -> u32) -> Self {
        Self {
            client: provider_client(),
            api_key,
            api_base: ANTHROPIC_API_BASE.into(),
            model,
            max_tokens,
            thinking_protocol: thinking::AnthropicThinkingProtocol::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_thinking_protocol(
        mut self,
        thinking_protocol: thinking::AnthropicThinkingProtocol,
    ) -> Self {
        self.thinking_protocol = thinking_protocol;
        self
    }

    pub(crate) fn new_with_transport(
        model: String,
        api_key: String,
        max_tokens: fn(&str) -> u32,
        client: reqwest::Client,
        api_base: String,
    ) -> Self {
        let thinking_protocol = thinking::resolve_thinking_protocol(&model);
        Self {
            client,
            api_key,
            api_base,
            model,
            max_tokens,
            thinking_protocol,
        }
    }

    fn request_body(
        &self,
        request: ModelRequest<'_>,
        stream: bool,
    ) -> Result<AnthropicRequest, ModelError> {
        let target = self.model_identity();
        let max_tokens = (self.max_tokens)(&self.model);
        let (thinking, output_config) = thinking::thinking_config_for(
            &self.thinking_protocol,
            request.reasoning_level,
            max_tokens,
        )?;
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
        _on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        self.send_messages_stream(request, on_event).await
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

fn mark_cache_control_points(messages: &mut [AnthropicMessage]) {
    // Writes occur only at marked breakpoints. When the last user message has a
    // trailing per-request suffix, mark the last shared cacheable block, not the
    // suffix. A single cacheable block is marked as before.
    for message in messages.iter_mut().rev() {
        if message.role != AnthropicRole::User {
            continue;
        }
        if mark_last_shared_user_breakpoint(&mut message.content) {
            return;
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
            *cache_control = Some(AnthropicCacheControl::ephemeral());
            return;
        }
    }
}

fn mark_last_shared_user_breakpoint(content: &mut [AnthropicContentBlock]) -> bool {
    let cacheable = content
        .iter()
        .enumerate()
        .filter(|(_, block)| is_cacheable_user_block(block))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let Some(&last) = cacheable.last() else {
        return false;
    };
    // A trailing user text block is the per-request suffix. Tool-result tails stay
    // marked so a multi-result user turn still writes the full prefix.
    let index = if cacheable.len() > 1 && is_user_text_block(&content[last]) {
        cacheable[cacheable.len() - 2]
    } else {
        last
    };
    match &mut content[index] {
        AnthropicContentBlock::Text { cache_control, .. }
        | AnthropicContentBlock::ToolResult { cache_control, .. } => {
            *cache_control = Some(AnthropicCacheControl::ephemeral());
            true
        }
        AnthropicContentBlock::Thinking { .. }
        | AnthropicContentBlock::RedactedThinking { .. }
        | AnthropicContentBlock::Image { .. }
        | AnthropicContentBlock::ToolUse { .. } => false,
    }
}

fn is_cacheable_user_block(block: &AnthropicContentBlock) -> bool {
    matches!(
        block,
        AnthropicContentBlock::Text { .. } | AnthropicContentBlock::ToolResult { .. }
    )
}

fn is_user_text_block(block: &AnthropicContentBlock) -> bool {
    matches!(block, AnthropicContentBlock::Text { .. })
}

crate::impl_sdk_model_provider!(AnthropicProvider);

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
