//! Provider-neutral model messages, requests, responses, and usage.
//!
//! These values are owned by the SDK rather than a specific transport. Message
//! serialization intentionally preserves Rho's historical externally-tagged
//! enum representation so existing session history remains readable.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const PORTABLE_FALLBACK_CONTEXT_KIND: &str = "rho.sdk.portable_fallback.v1";

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

/// Complete tool call requested by a model or supplied by the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    pub(crate) fn has_valid_protocol_fields(&self) -> bool {
        !self.id.is_empty() && !self.name.is_empty() && self.arguments.is_object()
    }
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
        !self.is_portable_fallback() && self.identity == *target
    }

    pub(crate) fn is_portable_fallback(&self) -> bool {
        self.kind == PORTABLE_FALLBACK_CONTEXT_KIND
            && self.position.is_none()
            && self.data.is_string()
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
            kind: PORTABLE_FALLBACK_CONTEXT_KIND.into(),
            position: None,
            data: Value::String(fallback.into()),
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Message {
    System(String),
    User(Vec<ContentBlock>),
    /// Legacy provider-neutral assistant format retained for session compatibility.
    Assistant(Vec<ContentBlock>),
    /// Assistant output with model provenance and portable/provider-owned context.
    EnrichedAssistant(Box<AssistantMessage>),
    /// Partial assistant output retained when a model turn ends incompletely.
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
    ProviderContext {
        kind: String,
        position: Option<usize>,
        data: Value,
    },
    Usage(ModelUsage),
}

/// Reserved [`ModelEvent::ProviderContext::kind`] for provider-native hosted
/// tool activity (for example xAI `x_search`).
///
/// Construct events with [`ModelEvent::hosted_tool_activity`]. The runtime maps
/// this kind to [`crate::RunEvent::HostedToolActivity`] and does not retain it
/// as provider-context replay state. This extension point exists because
/// [`ModelEvent`] is exhaustive in 1.x; a future major release may promote
/// hosted activity to a dedicated variant.
pub const HOSTED_TOOL_ACTIVITY_KIND: &str = "hosted_tool_activity";

/// Reserved [`ModelEvent::ProviderContext::kind`] for a completed turn that ran
/// on a different service tier than requested.
///
/// Construct events with [`ModelEvent::service_tier_fallback`]. The runtime maps
/// this kind to [`crate::RunEvent::ProviderServiceTierFallback`] and does not
/// retain it as provider-context replay state.
pub const SERVICE_TIER_FALLBACK_KIND: &str = "service_tier_fallback";
/// Reserved [`ModelEvent::ProviderContext::kind`] for provider-reported
/// non-reasoning output tokens.
///
/// Construct with [`ModelEvent::generation_output_tokens`]. The runtime consumes
/// this performance metadata before provider-context replay.
#[doc(hidden)]
pub const GENERATION_OUTPUT_TOKENS_KIND: &str = "rho_model_call_generation_output_tokens";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationOutputTokens {
    Reported(u64),
    Unavailable,
}

impl ModelEvent {
    /// Builds provider-native hosted tool activity for the stream.
    ///
    /// Carried as a reserved [`ModelEvent::ProviderContext`] kind so 1.x stays
    /// minor-compatible, then lowered to [`crate::RunEvent::HostedToolActivity`].
    pub fn hosted_tool_activity(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::ProviderContext {
            kind: HOSTED_TOOL_ACTIVITY_KIND.into(),
            position: None,
            data: json!({
                "name": name.into(),
                "detail": detail.into(),
            }),
        }
    }

    /// Returns hosted-tool activity when this event carries that reserved kind.
    pub fn as_hosted_tool_activity(&self) -> Option<(&str, &str)> {
        match self {
            Self::ProviderContext { kind, data, .. } if kind == HOSTED_TOOL_ACTIVITY_KIND => {
                let name = data.get("name")?.as_str()?;
                let detail = data.get("detail")?.as_str()?;
                Some((name, detail))
            }
            _ => None,
        }
    }

    /// Carries the exact non-reasoning output-token count through a 1.x provider callback.
    ///
    /// # Next major
    ///
    /// NEXT_MAJOR(rho-sdk): replace the generation-token ProviderContext carrier
    /// with a dedicated provider metric callback that does not pass through ModelEvent.
    ///
    /// This reserved context kind keeps the exhaustive 1.x [`ModelEvent`] enum
    /// source-compatible. The runtime consumes it as internal performance
    /// metadata and does not expose or persist it as provider context.
    #[doc(hidden)]
    pub fn generation_output_tokens(tokens: u64) -> Self {
        Self::ProviderContext {
            kind: GENERATION_OUTPUT_TOKENS_KIND.into(),
            position: None,
            data: json!({ "tokens": tokens }),
        }
    }

    /// Returns the state carried by the reserved generation-output metadata.
    pub(crate) fn as_generation_output_tokens(&self) -> Option<GenerationOutputTokens> {
        match self {
            Self::ProviderContext { kind, data, .. } if kind == GENERATION_OUTPUT_TOKENS_KIND => {
                Some(
                    data.get("tokens")
                        .and_then(serde_json::Value::as_u64)
                        .map_or(
                            GenerationOutputTokens::Unavailable,
                            GenerationOutputTokens::Reported,
                        ),
                )
            }
            _ => None,
        }
    }

    /// Builds a service-tier fallback observation for the stream.
    ///
    /// Carried as a reserved [`ModelEvent::ProviderContext`] kind so 1.x stays
    /// minor-compatible, then lowered to
    /// [`crate::RunEvent::ProviderServiceTierFallback`].
    pub fn service_tier_fallback(requested: ServiceTier, used: impl Into<String>) -> Self {
        let requested = match requested {
            ServiceTier::Priority => "priority",
        };
        Self::ProviderContext {
            kind: SERVICE_TIER_FALLBACK_KIND.into(),
            position: None,
            data: json!({
                "requested": requested,
                "used": used.into(),
            }),
        }
    }

    /// Returns service-tier fallback details when this event carries that kind.
    pub fn as_service_tier_fallback(&self) -> Option<(ServiceTier, &str)> {
        match self {
            Self::ProviderContext { kind, data, .. } if kind == SERVICE_TIER_FALLBACK_KIND => {
                let requested = match data.get("requested")?.as_str()? {
                    "priority" => ServiceTier::Priority,
                    _ => return None,
                };
                let used = data.get("used")?.as_str()?;
                Some((requested, used))
            }
            _ => None,
        }
    }
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
