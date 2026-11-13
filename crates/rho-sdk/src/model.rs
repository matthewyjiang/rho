//! Provider-neutral model messages, requests, responses, and usage.
//!
//! These values are owned by the SDK rather than a specific transport. Message
//! serialization intentionally preserves Rho's historical externally-tagged
//! enum representation so existing session history remains readable.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{tool::ToolExecutionMode, CancellationToken};

mod compaction_summary;
pub mod context;
pub mod handoff;
mod legacy_history;
mod legacy_tool_images;
mod message_time;
mod model_context;
mod provider_context;
mod reasoning_shift;
mod semantic;
mod stop_reason;
mod wire;

pub use compaction_summary::CompactionSummary;
pub use legacy_tool_images::attach_legacy_tool_images;
pub use message_time::MessageTime;
pub use provider_context::{ProviderContextBlock, ReplayScope};
pub(crate) use reasoning_shift::record as record_reasoning_shift;
pub use reasoning_shift::ReasoningShift;
pub use semantic::SemanticMessage;
pub use stop_reason::ModelStopReason;
pub use wire::WireMessage;

/// Provider-neutral specification for a tool available during a model turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Complete tool call requested by a model or supplied by the host.
///
/// `execution` records how the provider accepted the call; it is
/// [`ToolExecutionMode::Sync`] unless the provider reported
/// [`ModelEvent::ToolCallAccepted`]. Serialization omits the default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "ToolExecutionMode::is_sync")]
    pub execution: ToolExecutionMode,
}

impl ToolCall {
    /// A call the loop waits for before the next model request.
    pub fn new(id: String, name: String, arguments: Value) -> Self {
        Self {
            id,
            name,
            arguments,
            execution: ToolExecutionMode::Sync,
        }
    }

    pub(crate) fn has_valid_protocol_fields(&self) -> bool {
        !self.id.is_empty() && !self.name.is_empty() && self.arguments.is_object()
    }
}

/// Outcome a tool result reports to the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolResultStatus {
    Succeeded,
    Failed,
}

/// Result returned to a model after a tool call.
///
/// `images` are untrusted tool data, not user instructions. Provider
/// converters must deliver them: inside the tool result when the API accepts
/// that, and otherwise as attributed user content after the result.
///
/// Serialization writes `status` and omits empty `images`. Deserialization
/// also accepts the rho-sdk 5.x `ok: bool` shape; move 5.x tool-image user
/// messages onto their results with [`attach_legacy_tool_images`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolResult {
    pub id: String,
    pub status: ToolResultStatus,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageContent>,
}

impl ToolResult {
    /// A text-only result with `status`.
    pub fn new(
        id: impl Into<String>,
        status: ToolResultStatus,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            status,
            content: content.into(),
            images: Vec::new(),
        }
    }

    pub fn succeeded(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::new(id, ToolResultStatus::Succeeded, content)
    }

    pub fn failed(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::new(id, ToolResultStatus::Failed, content)
    }

    /// Replaces the images returned with this result.
    pub fn with_images(mut self, images: Vec<ImageContent>) -> Self {
        self.images = images;
        self
    }

    /// Whether the model should see this result as an error.
    pub fn is_failure(&self) -> bool {
        match self.status {
            ToolResultStatus::Succeeded => false,
            ToolResultStatus::Failed => true,
        }
    }
}

/// Accepts the current shape and the rho-sdk 5.x `ok: bool` shape.
#[derive(Deserialize)]
struct ToolResultRepr {
    id: String,
    #[serde(default)]
    status: Option<ToolResultStatus>,
    #[serde(default)]
    ok: Option<bool>,
    content: String,
    #[serde(default)]
    images: Vec<ImageContent>,
}

impl<'de> Deserialize<'de> for ToolResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let repr = ToolResultRepr::deserialize(deserializer)?;
        let status = match (repr.status, repr.ok) {
            (Some(status), _) => status,
            (None, Some(true)) => ToolResultStatus::Succeeded,
            (None, Some(false)) => ToolResultStatus::Failed,
            (None, None) => return Err(serde::de::Error::missing_field("status")),
        };
        Ok(Self {
            id: repr.id,
            status,
            content: repr.content,
            images: repr.images,
        })
    }
}

/// Tool call fragment retained when a model turn is interrupted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialToolCall {
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: String,
}

/// Partial assistant output retained after an incomplete model turn.
///
/// Used when a run ends cooperatively before the assistant finishes, including
/// explicit cancellation and terminal provider/run failure with streamed
/// progress. Provider adapters may append a model-visible abort marker when
/// replaying this entry on a later turn.
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

/// Completed assistant output with portable and provider-native context.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ModelIdentity>,
    /// Provider-produced reasoning summary. Raw reasoning must never be stored here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_summary: Option<String>,
    /// Opaque provider data filtered by [`ProviderContextBlock::is_replayable_to`].
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

    /// Adds portable text to use only when opaque provider context cannot replay.
    ///
    /// The fallback is stored as SDK metadata in [`Self::provider_context`] to
    /// preserve compatibility with existing `AssistantMessage` struct literals.
    /// It is never replayed as provider-native context.
    pub fn with_portable_fallback(mut self, fallback: impl Into<String>) -> Self {
        self.provider_context
            .retain(|block| !block.is_portable_fallback());
        let identity = self
            .provenance
            .clone()
            .unwrap_or_else(|| ModelIdentity::new("", "", ""));
        self.provider_context.push(ProviderContextBlock {
            identity,
            kind: provider_context::PORTABLE_FALLBACK_CONTEXT_KIND.into(),
            position: None,
            data: Value::String(fallback.into()),
            replay: ReplayScope::ExactModel,
        });
        self
    }

    /// Returns portable fallback text attached by [`Self::with_portable_fallback`].
    pub fn portable_fallback(&self) -> Option<&str> {
        self.provider_context
            .iter()
            .find(|block| block.is_portable_fallback())
            .and_then(|block| block.data.as_str())
    }

    /// Removes provider-native context while retaining portable SDK metadata.
    pub fn retain_portable_context(&mut self) {
        self.provider_context
            .retain(ProviderContextBlock::is_portable_fallback);
    }
}

#[derive(Deserialize)]
struct AssistantMessageRepr {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    provenance: Option<ModelIdentity>,
    #[serde(default)]
    reasoning_summary: Option<String>,
    #[serde(default)]
    portable_fallback: Option<String>,
    #[serde(default)]
    provider_context: Vec<ProviderContextBlock>,
}

impl<'de> Deserialize<'de> for AssistantMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let repr = AssistantMessageRepr::deserialize(deserializer)?;
        let message = Self {
            content: repr.content,
            provenance: repr.provenance,
            reasoning_summary: repr.reasoning_summary,
            provider_context: repr.provider_context,
        };
        Ok(match repr.portable_fallback {
            Some(fallback) => message.with_portable_fallback(fallback),
            None => message,
        })
    }
}

/// One provider-neutral history entry.
///
/// Tool-owned images ride on [`ToolResult::images`]. Provider converters send
/// entries through [`Self::wire`]; classify meaning with [`Self::semantic`].
///
/// Exhaustive on purpose, like [`ContentBlock`]: every provider converter must
/// decide how to send each variant, so a new one is a breaking change rather
/// than a silently dropped history entry.
///
/// Deserialization converts rho-sdk 5.x encodings (user-role compaction
/// summaries and model context, async-call marker blocks) to these variants.
///
/// Content payloads are reference counted, so cloning a message, or a
/// history, shares its content instead of copying it. A session's history is
/// cloned for every run, checkpoint and save, and hosts keep further copies,
/// so a deep clone multiplied a session's memory by the copies held. Change a
/// shared payload with [`Arc::make_mut`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "MessageRepr")]
pub enum Message {
    System(String),
    User(Arc<[ContentBlock]>),
    /// Legacy provider-neutral assistant format retained for session compatibility.
    Assistant(Arc<[ContentBlock]>),
    /// Assistant output with model provenance and portable/provider-owned context.
    EnrichedAssistant(Arc<AssistantMessage>),
    /// Partial assistant output retained when a model turn ends incompletely.
    AbortedAssistant(Arc<AbortedAssistant>),
    ToolResult(Arc<ToolResult>),
    /// Model-written summary that replaced earlier history.
    CompactionSummary(CompactionSummary),
    /// Request-only host context: data, not instructions. Built by
    /// [`crate::RequestContext`] sources; never committed to history.
    ModelContext(String),
    /// Host input accepted at a runtime boundary (see
    /// [`crate::BoundaryInputRequest::respond`]), not human input.
    BoundaryInput(Arc<[ContentBlock]>),
    /// The session's reasoning level changed before the next entry.
    ReasoningShift(ReasoningShift),
}

/// Serialized shape of [`Message`]; deserialization upgrades 5.x encodings.
#[derive(Deserialize)]
enum MessageRepr {
    System(String),
    User(Arc<[ContentBlock]>),
    Assistant(Arc<[ContentBlock]>),
    EnrichedAssistant(Arc<AssistantMessage>),
    AbortedAssistant(Arc<AbortedAssistant>),
    ToolResult(Arc<ToolResult>),
    CompactionSummary(CompactionSummary),
    ModelContext(String),
    BoundaryInput(Arc<[ContentBlock]>),
    ReasoningShift(ReasoningShift),
}

impl From<MessageRepr> for Message {
    fn from(repr: MessageRepr) -> Self {
        let message = match repr {
            MessageRepr::System(text) => Self::System(text),
            MessageRepr::User(content) => Self::User(content),
            MessageRepr::Assistant(content) => Self::Assistant(content),
            MessageRepr::EnrichedAssistant(message) => Self::EnrichedAssistant(message),
            MessageRepr::AbortedAssistant(message) => Self::AbortedAssistant(message),
            MessageRepr::ToolResult(result) => Self::ToolResult(result),
            MessageRepr::CompactionSummary(summary) => Self::CompactionSummary(summary),
            MessageRepr::ModelContext(text) => Self::ModelContext(text),
            MessageRepr::BoundaryInput(content) => Self::BoundaryInput(content),
            MessageRepr::ReasoningShift(shift) => Self::ReasoningShift(shift),
        };
        legacy_history::upgrade_message(message)
    }
}

impl Message {
    pub fn user_text(content: impl Into<String>) -> Self {
        Self::User(vec![ContentBlock::Text(content.into())].into())
    }

    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self::Assistant(vec![ContentBlock::Text(content.into())].into())
    }

    pub fn assistant(message: AssistantMessage) -> Self {
        Self::EnrichedAssistant(Arc::new(message))
    }

    /// Construct a compaction summary for replacement history.
    pub fn compaction_summary(trigger: crate::CompactionTrigger, text: impl Into<String>) -> Self {
        Self::CompactionSummary(CompactionSummary::new(trigger, text))
    }

    pub fn completed_assistant_content(&self) -> Option<&[ContentBlock]> {
        match self {
            Self::Assistant(content) => Some(content),
            Self::EnrichedAssistant(message) => Some(&message.content),
            Self::System(_)
            | Self::User(_)
            | Self::AbortedAssistant(_)
            | Self::ToolResult(_)
            | Self::CompactionSummary(_)
            | Self::ModelContext(_)
            | Self::BoundaryInput(_)
            | Self::ReasoningShift(_) => None,
        }
    }
}

/// One provider-neutral content item.
///
/// Exhaustive on purpose: every provider converter must decide how to send
/// each block, so a new one is a breaking change rather than silently dropped
/// request content.
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

impl ImageContent {
    /// Detects PNG, JPEG, GIF, or WebP from leading magic bytes.
    pub fn mime_type_from_bytes(header: &[u8]) -> Option<&'static str> {
        if header.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some("image/png")
        } else if header.starts_with(&[0xff, 0xd8, 0xff]) {
            Some("image/jpeg")
        } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
            Some("image/gif")
        } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP") {
            Some("image/webp")
        } else {
            None
        }
    }
}

/// Provider service class requested for one model turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ServiceTier {
    /// Prefer the provider's low-latency priority service.
    Priority,
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

impl ModelResponse {
    /// Explains why this response violates the tool-call protocol, if it does.
    ///
    /// Orchestration rejects responses with an issue and surfaces the returned
    /// message. The per-call checks mirror [`ToolCall::has_valid_protocol_fields`]
    /// and the walk adds duplicate-id detection across the whole response.
    pub(crate) fn protocol_issue(&self) -> Option<String> {
        let ModelResponse::Assistant(content) = self;
        if content.is_empty() {
            return Some("provider returned an empty assistant response".into());
        }
        let mut issues = Vec::new();
        let mut call_ids = std::collections::BTreeSet::new();
        for (index, block) in content.iter().enumerate() {
            let ContentBlock::ToolCall(call) = block else {
                continue;
            };
            if call.id.is_empty() {
                issues.push(format!("tool call {index} has an empty id"));
            } else if !call_ids.insert(call.id.as_str()) {
                issues.push(format!("duplicate tool call id '{}'", call.id));
            }
            if call.name.is_empty() {
                issues.push(format!("tool call {index} has an empty name"));
            }
            if !call.arguments.is_object() {
                issues.push(format!("tool call {index} arguments are not a JSON object"));
            }
        }
        if issues.is_empty() {
            None
        } else {
            Some(format!(
                "provider returned a malformed assistant response: {}",
                issues.join("; ")
            ))
        }
    }
}

/// Host-reported usage where the prompt total still includes cache hits.
///
/// Pass this to [`ModelUsage::from_inclusive_prompt`]. Anthropic-style hosts
/// that already report an uncached remainder should construct [`ModelUsage`]
/// directly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InclusivePromptUsage {
    pub prompt_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reported_total: Option<u64>,
    pub context_window: Option<u64>,
    pub cost_usd_micros: Option<u64>,
}

/// Normalized token, context, and cost accounting for model work.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    /// Uncached input tokens charged at the normal input-token rate.
    ///
    /// Absent when the host has not reported a cache split. Do not treat a
    /// missing value as zero uncached tokens, and do not store a mixed prompt
    /// total here. Use [`Self::from_inclusive_prompt`] for inclusive-prompt
    /// hosts and [`Self::inclusive_prompt_tokens`] for prompt size.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub cost_usd_micros: Option<u64>,
}

impl ModelUsage {
    /// Build usage from a host that reports an inclusive prompt total.
    ///
    /// Uncached input is derived only when a cache field is present, including
    /// an explicit zero. A missing cache count is not treated as zero cache.
    /// `total_tokens` keeps the host total, or prompt plus output when the host
    /// omitted it, so [`Self::inclusive_prompt_tokens`] can recover prompt size.
    pub fn from_inclusive_prompt(usage: InclusivePromptUsage) -> Self {
        let input_tokens = match (
            usage.prompt_tokens,
            usage.cache_read_tokens,
            usage.cache_write_tokens,
        ) {
            (Some(prompt), cache_read, cache_write)
                if cache_read.is_some() || cache_write.is_some() =>
            {
                Some(
                    prompt
                        .saturating_sub(cache_read.unwrap_or_default())
                        .saturating_sub(cache_write.unwrap_or_default()),
                )
            }
            _ => None,
        };
        let total_tokens =
            usage
                .reported_total
                .or_else(|| match (usage.prompt_tokens, usage.output_tokens) {
                    (Some(prompt), Some(output)) => Some(prompt.saturating_add(output)),
                    (Some(prompt), None) => Some(prompt),
                    _ => None,
                });
        Self {
            input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            total_tokens,
            context_window: usage.context_window,
            cost_usd_micros: usage.cost_usd_micros,
        }
    }

    /// Sum of the disjoint input buckets: uncached, cache read, and cache write.
    ///
    /// Missing when every bucket is absent. This is not prompt size for a mixed
    /// total; use [`Self::inclusive_prompt_tokens`] for that.
    pub fn total_input_tokens(&self) -> Option<u64> {
        let has_input = self.input_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_write_tokens.is_some();
        has_input.then_some(
            self.input_tokens
                .unwrap_or_default()
                .saturating_add(self.cache_read_tokens.unwrap_or_default())
                .saturating_add(self.cache_write_tokens.unwrap_or_default()),
        )
    }

    /// Prompt tokens present in the request, including cache hits and writes.
    ///
    /// Prefers the disjoint buckets. When those undercount an accumulated
    /// session, or when they are absent, uses `total_tokens` minus output so
    /// context fill and catalog cost can still see a prompt. Returns none when
    /// output is unknown: a bare total may still be growing with generation.
    pub fn inclusive_prompt_tokens(&self) -> Option<u64> {
        let buckets = self.total_input_tokens();
        let recovered = match (self.total_tokens, self.output_tokens) {
            (Some(total), Some(output)) => Some(total.saturating_sub(output)),
            _ => None,
        };
        match (buckets, recovered) {
            (Some(known), Some(from_total)) => Some(known.max(from_total)),
            (Some(known), None) | (None, Some(known)) => Some(known),
            (None, None) => None,
        }
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
    /// Provider-native web search activity observed during a model turn.
    WebSearch(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    /// Opaque provider-native data the runtime stores on the assistant turn as
    /// a [`ProviderContextBlock`] tagged with the provider's identity.
    ProviderContext {
        kind: String,
        position: Option<usize>,
        data: Value,
        replay: ReplayScope,
    },
    /// The provider accepted tool call `id` with `execution`. The runtime
    /// records it on [`ToolCall::execution`]; send it before returning the
    /// response. Only [`ToolExecutionMode::Async`] changes scheduling.
    ToolCallAccepted {
        id: String,
        execution: ToolExecutionMode,
    },
    /// Why the provider ended this turn early. Send it once the reason is
    /// known; if a turn reports more than once, the last report wins. The
    /// runtime consumes it; it never reaches history.
    Stopped(ModelStopReason),
    Usage(ModelUsage),
    /// Generation-window output tokens for this attempt.
    ///
    /// The runtime consumes this into [`crate::ModelCallMetrics`] and never
    /// lowers it to a [`crate::RunEvent`] or persists it as provider context.
    GenerationOutputTokens(GenerationOutputTokens),
    /// Provider-native hosted tool activity (for example xAI `x_search`).
    ///
    /// The runtime maps this to [`crate::RunEvent::HostedToolActivity`] and
    /// does not retain it as provider-context replay state.
    HostedToolActivity {
        name: String,
        detail: String,
    },
    /// The provider completed a request on a different service tier.
    ///
    /// The runtime maps this to [`crate::RunEvent::ProviderServiceTierFallback`]
    /// and does not retain it as provider-context replay state.
    ServiceTierFallback {
        requested: ServiceTier,
        used: String,
    },
}

/// Generation-window output tokens reported by a provider for one attempt.
///
/// Distinct from aggregate [`ModelUsage::output_tokens`], which keeps the
/// provider's full billed output total (including reasoning).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationOutputTokens {
    /// Non-reasoning output count that matches the generation timing window.
    Reported(u64),
    /// The provider produced output that cannot be attributed to the generation
    /// window (hidden reasoning without a token split, or a burst-replayed
    /// proxy flush whose generation window measures replay speed).
    Unavailable,
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
            .inclusive_prompt_tokens()
            .map(|tokens| Self::provider_reported(tokens, usage.context_window))
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
