use std::time::Duration;

use crate::{
    model::{ContentBlock, ModelUsage, ToolCall},
    tool::{ToolErrorKind, ToolMetadata, ToolOutput, ToolProgress},
    Revision, RunId, SteeringId, ToolCallId,
};

/// Legacy provider activity kind emitted when a malformed response is retried.
///
/// Prefer [`RunEvent::ProviderStreamReset`]. Still emitted before the typed reset
/// for 1.0 hosts.
///
/// NEXT_MAJOR(rho-sdk): remove ProviderActivity and PROVIDER_ACTIVITY_* dual-emits.
#[deprecated(since = "1.11.0", note = "use RunEvent::ProviderStreamReset")]
pub const PROVIDER_ACTIVITY_INVALID_RESPONSE_RETRY: &str = "invalid_response_retry";
/// Legacy provider activity kind emitted when a physical provider request is retried.
///
/// Prefer [`RunEvent::ProviderRequestRetry`]. Still dual-emitted for 1.0 hosts.
///
/// NEXT_MAJOR(rho-sdk): remove ProviderActivity and PROVIDER_ACTIVITY_* dual-emits.
#[deprecated(since = "1.11.0", note = "use RunEvent::ProviderRequestRetry")]
pub const PROVIDER_ACTIVITY_REQUEST_RETRY: &str = "provider_request_retry";
/// Legacy provider activity kind emitted for provider-native web searches.
///
/// Prefer [`RunEvent::WebSearch`]. Still dual-emitted for 1.0 hosts.
///
/// NEXT_MAJOR(rho-sdk): remove ProviderActivity and PROVIDER_ACTIVITY_* dual-emits.
#[deprecated(since = "1.11.0", note = "use RunEvent::WebSearch")]
pub const PROVIDER_ACTIVITY_WEB_SEARCH: &str = "web_search";

/// Why the current provider attempt was abandoned before a fresh request.
///
/// # Next major
///
/// NEXT_MAJOR(rho-sdk): collapse RetryableFailure and RetryableFailureWithRetryAfter
/// into one shape with optional retry_after (or move retry_after onto
/// [`RunEvent::ProviderStreamReset`]).
///
/// Wait is metadata on a retryable failure, not a distinct reason. The split exists
/// only so a minor release can carry the wait without adding a field to
/// [`RunEvent::ProviderStreamReset`]. Prefer matching via
/// [`Self::provider_error_kind`] / [`Self::retry_after`] until major so both arms
/// stay covered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProviderStreamResetReason {
    /// The provider returned a malformed normalized assistant response.
    InvalidResponse,
    /// The provider request failed with a retryable error (no wait hint).
    RetryableFailure(crate::ProviderErrorKind),
    /// Same as [`Self::RetryableFailure`], with a provider-supplied wait hint.
    ///
    /// Exists only so wait metadata can land in a minor release without adding a
    /// field to [`RunEvent::ProviderStreamReset`]. Prefer
    /// [`Self::retryable_failure`] when constructing and
    /// [`Self::provider_error_kind`] / [`Self::retry_after`] when matching.
    /// See the enum-level next-major note.
    RetryableFailureWithRetryAfter {
        kind: crate::ProviderErrorKind,
        retry_after: Duration,
    },
}

impl ProviderStreamResetReason {
    /// Builds a retryable-failure reason, attaching a wait when the provider supplied one.
    ///
    /// Prefer this over constructing [`Self::RetryableFailure`] /
    /// [`Self::RetryableFailureWithRetryAfter`] directly.
    pub fn retryable_failure(
        kind: crate::ProviderErrorKind,
        retry_after: Option<Duration>,
    ) -> Self {
        match retry_after.filter(|delay| !delay.is_zero()) {
            Some(retry_after) => Self::RetryableFailureWithRetryAfter { kind, retry_after },
            None => Self::RetryableFailure(kind),
        }
    }

    /// Provider error kind when this reset was caused by a retryable failure.
    ///
    /// Covers both [`Self::RetryableFailure`] and
    /// [`Self::RetryableFailureWithRetryAfter`].
    pub fn provider_error_kind(self) -> Option<crate::ProviderErrorKind> {
        match self {
            Self::InvalidResponse => None,
            Self::RetryableFailure(kind) | Self::RetryableFailureWithRetryAfter { kind, .. } => {
                Some(kind)
            }
        }
    }

    /// Provider-supplied wait before the next attempt may succeed.
    pub fn retry_after(self) -> Option<Duration> {
        match self {
            Self::RetryableFailureWithRetryAfter { retry_after, .. } => Some(retry_after),
            Self::InvalidResponse | Self::RetryableFailure(_) => None,
        }
    }
}

/// Reason a successful run stopped producing model turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopReason {
    EndTurn,
    /// The configured model-step budget was exhausted after committing progress.
    MaxSteps,
}

/// Final typed result of a successful run.
#[derive(Clone, Debug, PartialEq)]
pub struct RunOutcome {
    content: Vec<ContentBlock>,
    text: String,
    usage: ModelUsage,
    stop_reason: StopReason,
    revision: Revision,
}

impl RunOutcome {
    pub(crate) fn new(
        content: Vec<ContentBlock>,
        usage: ModelUsage,
        stop_reason: StopReason,
        revision: Revision,
    ) -> Self {
        let text = content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
            })
            .collect::<Vec<_>>()
            .join("");
        Self {
            content,
            text,
            usage,
            stop_reason,
            revision,
        }
    }

    pub fn content(&self) -> &[ContentBlock] {
        &self.content
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn usage(&self) -> &ModelUsage {
        &self.usage
    }

    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }
}

/// Structured tool failure included in a completed tool event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolFailure {
    kind: ToolErrorKind,
    message: String,
}

impl ToolFailure {
    pub(crate) fn new(kind: ToolErrorKind, message: String) -> Self {
        Self { kind, message }
    }

    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Result included in [`RunEvent::ToolFinished`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolCompletion {
    Success(ToolOutput),
    Failure(ToolFailure),
    Unavailable,
}

/// Provider and request settings that affect model-call performance.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModelCallProfile {
    /// Provider identifier used for this call.
    pub provider: String,
    /// Model identifier used for this call.
    pub model: String,
    /// Reasoning level used for this call.
    pub reasoning: crate::ReasoningLevel,
    /// Requested provider service class, if any.
    pub service_tier: Option<crate::model::ServiceTier>,
}

/// Timing and provider-reported output usage for one model call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelCallMetrics {
    /// Provider-reported output tokens for this call.
    pub output_tokens: Option<u64>,
    /// Time from the start of the attempt to its first generated event.
    /// Reasoning deltas count as generated output, so a provider that hides
    /// reasoning until the visible answer begins reports that whole wait here.
    pub time_to_first_token: Option<Duration>,
    /// Time from the first generated event until stream completion.
    pub generation_time: Option<Duration>,
    /// Time from the start of the attempt until stream completion.
    ///
    /// Every duration here is scoped to the attempt that produced the returned
    /// output. Discarded attempts and the retry backoff before them are not
    /// counted, so these numbers describe the model rather than retry policy.
    pub total_latency: Duration,
}

impl ModelCallMetrics {
    /// Provider-reported output tokens divided by total attempt latency.
    ///
    /// Total latency rather than generation time keeps the time boundary
    /// aligned with providers that count hidden reasoning in `output_tokens`
    /// before emitting their first event.
    pub fn output_tokens_per_second(self) -> Option<f64> {
        let tokens = self.output_tokens?;
        let seconds = self.total_latency.as_secs_f64();
        (seconds > 0.0).then(|| tokens as f64 / seconds)
    }
}

/// Ordered semantic event emitted during a run.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum RunEvent {
    Started {
        run_id: RunId,
        revision: Revision,
    },
    StepStarted {
        step: usize,
        /// Provider-neutral token estimate of the request history and tool
        /// schemas for this step. Hosts should treat this as a display estimate
        /// and replace it when [`RunEvent::UsageUpdated`] reports provider input.
        estimated_context_tokens: u64,
    },
    AssistantTextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ReasoningSummaryDelta {
        text: String,
    },
    ToolCallUpdated {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
    ToolProposed {
        call: ToolCall,
    },
    ToolStarted {
        call_id: ToolCallId,
        name: String,
        metadata: ToolMetadata,
    },
    ToolUpdated {
        call_id: ToolCallId,
        progress: ToolProgress,
    },
    ToolFinished {
        call_id: ToolCallId,
        result: ToolCompletion,
    },
    UsageUpdated {
        usage: ModelUsage,
    },
    /// Legacy stringly-typed provider activity.
    ///
    /// Prefer the typed events instead. Still dual-emitted alongside
    /// [`RunEvent::WebSearch`], [`RunEvent::ProviderRequestRetry`], and
    /// [`RunEvent::ProviderStreamReset`] for 1.0 hosts. New activity such as
    /// [`RunEvent::HostedToolActivity`] is typed-only and does not dual-emit
    /// here.
    ///
    /// NEXT_MAJOR(rho-sdk): remove ProviderActivity and PROVIDER_ACTIVITY_* dual-emits.
    #[deprecated(
        since = "1.11.0",
        note = "use WebSearch, ProviderRequestRetry, or ProviderStreamReset"
    )]
    ProviderActivity {
        kind: String,
        detail: String,
    },
    ProviderContextUpdated {
        kind: String,
    },
    HostInputRequested {
        request: crate::HostInputRequest,
    },
    CompactionStarted {
        trigger: crate::CompactionTrigger,
        message_count: usize,
    },
    CompactionCompleted {
        trigger: crate::CompactionTrigger,
        outcome: crate::CompactionOutcome,
    },
    Completed {
        outcome: RunOutcome,
    },
    Cancelled {
        revision: Revision,
    },
    Failed {
        message: String,
        retryability: crate::Retryability,
    },
    /// Accepted steering crossed into conversation history for the next model step.
    SteeringApplied {
        ids: Vec<SteeringId>,
    },
    /// Provider details for direct user diagnostics only. This may contain
    /// provider-returned data and must not be added to model context.
    ProviderDiagnostic {
        detail: crate::ProviderDiagnostic,
    },
    /// The current provider attempt was abandoned. Hosts rendering live
    /// deltas must discard that attempt before processing subsequent deltas.
    ProviderStreamReset {
        reason: ProviderStreamResetReason,
        detail: String,
    },
    /// Host input requested by a correlated tool call.
    ToolHostInputRequested {
        call_id: ToolCallId,
        request: crate::HostInputRequest,
    },
    /// Provider-native web search activity observed during a model turn.
    ///
    /// Appended after existing variants so discriminant values of the 1.0
    /// surface stay stable under a minor release.
    WebSearch {
        detail: String,
    },
    /// A physical provider request failed and will be retried.
    ///
    /// Appended after existing variants so discriminant values of the 1.0
    /// surface stay stable under a minor release.
    ProviderRequestRetry,
    /// A model call completed with local timing and provider-reported usage.
    ///
    /// Appended after existing variants so discriminant values of the 1.0
    /// surface stay stable under a minor release.
    ModelCallCompleted {
        profile: ModelCallProfile,
        metrics: ModelCallMetrics,
    },
    /// Provider-native hosted tool activity observed during a model turn.
    ///
    /// `name` is the hosted tool id (for example `x_search`). Distinct from
    /// client-executed tools and from the historical [`RunEvent::WebSearch`]
    /// path. Appended after existing variants so discriminant values of the
    /// 1.0 surface stay stable under a minor release.
    HostedToolActivity {
        name: String,
        detail: String,
    },
    /// The provider completed a request on a different service tier.
    ProviderServiceTierFallback {
        requested: crate::model::ServiceTier,
        used: String,
    },
}

#[cfg(test)]
#[path = "event_tests.rs"]
mod tests;
