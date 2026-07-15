//! Provider-neutral model messages, requests, responses, and usage.
//!
//! These values are owned by the SDK rather than a specific transport. Message
//! serialization intentionally preserves Rho's historical externally-tagged
//! enum representation so existing session history remains readable.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::CancellationToken;

pub mod context;
pub mod handoff;

/// Provider-neutral specification for a tool available during a model turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Complete tool call requested by a model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Result returned to a model after a tool call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub ok: bool,
    pub content: String,
}

/// Tool call fragment retained when a model turn is interrupted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialToolCall {
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: String,
}

/// Partial assistant output retained after explicit cancellation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AbortedAssistant {
    pub content: Vec<ContentBlock>,
    pub reasoning: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ModelIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_context: Vec<ProviderContextBlock>,
    pub tool_calls: Vec<PartialToolCall>,
    pub usage: ModelUsage,
}

/// Exact provider, API, and model identity for replay-sensitive context.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelIdentity {
    pub provider: String,
    pub api: String,
    pub model: String,
}

impl ModelIdentity {
    pub fn new(
        provider: impl Into<String>,
        api: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            api: api.into(),
            model: model.into(),
        }
    }
}

/// Opaque provider-native data scoped to an exact model identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderContextBlock {
    pub identity: ModelIdentity,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
    pub data: Value,
}

impl ProviderContextBlock {
    pub fn is_replayable_to(&self, target: &ModelIdentity) -> bool {
        self.identity == *target
    }
}

/// Completed assistant output with portable and provider-native context.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ModelIdentity>,
    /// Provider-produced reasoning summary. Raw reasoning must never be stored here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_summary: Option<String>,
    /// Opaque provider data retained only for exact provider/API/model replay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_context: Vec<ProviderContextBlock>,
}

impl AssistantMessage {
    pub fn from_content(content: Vec<ContentBlock>) -> Self {
        Self {
            content,
            ..Self::default()
        }
    }
}

/// One provider-neutral history entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Message {
    System(String),
    User(Vec<ContentBlock>),
    /// Legacy provider-neutral assistant format retained for session compatibility.
    Assistant(Vec<ContentBlock>),
    /// Assistant output with model provenance and portable/provider-owned context.
    EnrichedAssistant(Box<AssistantMessage>),
    /// Partial assistant output retained when the run is explicitly cancelled.
    AbortedAssistant(Box<AbortedAssistant>),
    ToolResult(ToolResult),
}

impl Message {
    pub fn user_text(content: impl Into<String>) -> Self {
        Self::User(vec![ContentBlock::Text(content.into())])
    }

    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self::Assistant(vec![ContentBlock::Text(content.into())])
    }

    pub fn assistant(message: AssistantMessage) -> Self {
        Self::EnrichedAssistant(Box::new(message))
    }

    pub fn completed_assistant_content(&self) -> Option<&[ContentBlock]> {
        match self {
            Self::Assistant(content) => Some(content),
            Self::EnrichedAssistant(message) => Some(&message.content),
            Self::System(_) | Self::User(_) | Self::AbortedAssistant(_) | Self::ToolResult(_) => {
                None
            }
        }
    }
}

/// One provider-neutral content item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ContentBlock {
    Text(String),
    Image(ImageContent),
    ToolCall(ToolCall),
}

/// Base64-encoded image input and its media type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageContent {
    pub data: String,
    pub mime_type: String,
}

/// Borrowed input for one provider turn.
#[derive(Clone, Debug)]
pub struct ModelRequest<'a> {
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    pub cancellation: CancellationToken,
    pub reasoning_level: crate::ReasoningLevel,
    /// Provider-specific prompt cache key metadata.
    ///
    /// Providers must opt in explicitly when their API supports this field.
    pub prompt_cache_key: Option<&'a str>,
}

/// Normalized result of one provider turn.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelResponse {
    Assistant(Vec<ContentBlock>),
}

/// Normalized token, context, and cost accounting for model work.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    /// Uncached input tokens charged at the normal input-token rate.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub cost_usd_micros: Option<u64>,
}

impl ModelUsage {
    /// Input tokens present in the request, including cache hits and writes.
    pub fn total_input_tokens(&self) -> Option<u64> {
        let has_input = self.input_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_write_tokens.is_some();
        let total = self
            .input_tokens
            .unwrap_or_default()
            .saturating_add(self.cache_read_tokens.unwrap_or_default())
            .saturating_add(self.cache_write_tokens.unwrap_or_default());
        has_input.then_some(total)
    }

    /// Saturating sum used to accumulate usage across model steps.
    pub fn saturating_add(&self, other: &Self) -> Self {
        Self {
            input_tokens: add_optional(self.input_tokens, other.input_tokens),
            output_tokens: add_optional(self.output_tokens, other.output_tokens),
            cache_read_tokens: add_optional(self.cache_read_tokens, other.cache_read_tokens),
            cache_write_tokens: add_optional(self.cache_write_tokens, other.cache_write_tokens),
            total_tokens: add_optional(self.total_tokens, other.total_tokens),
            context_window: other.context_window.or(self.context_window),
            cost_usd_micros: add_optional(self.cost_usd_micros, other.cost_usd_micros),
        }
    }
}

fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (None, None) => None,
        (left, right) => Some(
            left.unwrap_or_default()
                .saturating_add(right.unwrap_or_default()),
        ),
    }
}

/// Semantic event produced while a provider response is streaming.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelEvent {
    OutputDelta(String),
    ReasoningDelta(String),
    /// A provider-produced reasoning summary safe to persist and hand off.
    ReasoningSummaryDelta(String),
    WebSearch(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    ProviderContext {
        kind: String,
        position: Option<usize>,
        data: Value,
    },
    Usage(ModelUsage),
}

/// Source used to calculate the current context consumption.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextUsageSource {
    Estimated,
    ProviderReported,
    UnknownAfterCompaction,
}

/// Current model-context consumption and its provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub source: ContextUsageSource,
}

impl ContextUsage {
    pub fn estimated(tokens: u64, context_window: Option<u64>) -> Self {
        Self {
            tokens: Some(tokens),
            context_window,
            source: ContextUsageSource::Estimated,
        }
    }

    pub fn provider_reported(tokens: u64, context_window: Option<u64>) -> Self {
        Self {
            tokens: Some(tokens),
            context_window,
            source: ContextUsageSource::ProviderReported,
        }
    }

    pub fn unknown_after_compaction(context_window: Option<u64>) -> Self {
        Self {
            tokens: None,
            context_window,
            source: ContextUsageSource::UnknownAfterCompaction,
        }
    }

    pub fn from_model_usage(usage: &ModelUsage) -> Option<Self> {
        usage
            .total_input_tokens()
            .map(|tokens| Self::provider_reported(tokens, usage.context_window))
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
