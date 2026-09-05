use std::time::Duration;

use crate::{
    model::{ContentBlock, GenerationOutputTokens, ModelUsage, ToolCall},
    tool::{ToolErrorKind, ToolMetadata, ToolOutput, ToolProgress},
    Revision, RunId, SteeringId, ToolCallId,
};

/// Why the current provider attempt was abandoned before a fresh request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProviderStreamResetReason {
    /// The provider returned a malformed normalized assistant response.
    InvalidResponse,
    /// The provider request failed with a retryable error.
    RetryableFailure {
        kind: crate::ProviderErrorKind,
        /// Provider-supplied wait before the next attempt may succeed.
        retry_after: Option<Duration>,
    },
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
    /// Provider-reported aggregate output tokens for this call.
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
    /// Generation-window output tokens when the provider reported a usable
    /// breakdown. `None` falls back to aggregate [`Self::output_tokens`] via
    /// [`Self::resolved_generation_tokens`]. [`GenerationOutputTokens::Unavailable`]
    /// means the provider produced output that cannot be attributed to the
    /// generation window.
    pub generation_output_tokens: Option<GenerationOutputTokens>,
}

impl ModelCallMetrics {
    /// Output tokens that match the generation timing window.
    ///
    /// `None` [`Self::generation_output_tokens`] falls back to aggregate
    /// [`Self::output_tokens`]. [`GenerationOutputTokens::Unavailable`]
    /// suppresses a count rather than falling back.
    pub fn resolved_generation_tokens(self) -> Option<u64> {
        match self.generation_output_tokens {
            None => self.output_tokens,
            Some(GenerationOutputTokens::Reported(tokens)) => Some(tokens),
            Some(GenerationOutputTokens::Unavailable) => None,
        }
    }

    /// Generation-window tokens divided by generation time.
    ///
    /// Generation time runs from the first generated event to stream end, so
    /// this matches common throughput definitions that exclude time to first
    /// token. Returns `None` when the call never streamed generated output or
    /// when [`Self::resolved_generation_tokens`] is `None`.
    pub fn generation_tokens_per_second(self) -> Option<f64> {
        Self::rate(self.resolved_generation_tokens(), self.generation_time?)
    }

    /// Provider-reported output tokens divided by total attempt latency.
    ///
    /// Total latency includes time to first token, so this is the end-to-end
    /// *response* rate rather than decode/throughput. Prefer
    /// [`Self::generation_tokens_per_second`] for generation-window rates.
    /// This form stays useful when hidden pre-stream work is charged in
    /// `output_tokens` before any event is emitted.
    ///
    /// Host surfaces (statusline average, primary `/info` rate) use generation
    /// throughput; keep this helper for last-call response comparison.
    pub fn response_tokens_per_second(self) -> Option<f64> {
        Self::rate(self.output_tokens, self.total_latency)
    }

    fn rate(tokens: Option<u64>, window: Duration) -> Option<f64> {
        let tokens = tokens?;
        let seconds = window.as_secs_f64();
        (seconds > 0.0).then(|| tokens as f64 / seconds)
    }
}

/// Ordered semantic event emitted during a run.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum RunEvent {
    /// Internal host input accepted at a runtime checkpoint, not human steering.
    BoundaryInputApplied {
        session_id: crate::SessionId,
        run_id: RunId,
        input: crate::UserInput,
    },
    Started {
        run_id: RunId,
        revision: Revision,
    },
    StepStarted {
        step: usize,
        /// Provider-neutral estimate of request history and tool schemas.
        /// Hosts should treat this as a display estimate and replace it with
        /// [`RunEvent::UsageUpdated`] when the provider reports input usage.
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
    /// Terminal run failure after a cooperative history commit.
    Failed {
        message: String,
        retryability: crate::Retryability,
        revision: Revision,
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
    WebSearch {
        detail: String,
    },
    /// A physical provider request failed and will be retried.
    ProviderRequestRetry,
    /// A model call completed with local timing and provider-reported usage.
    ModelCallCompleted {
        profile: ModelCallProfile,
        metrics: ModelCallMetrics,
    },
    /// Provider-native hosted tool activity observed during a model turn.
    ///
    /// `name` is the hosted tool id (for example `x_search`). Distinct from
    /// client-executed tools and from the historical [`RunEvent::WebSearch`]
    /// path.
    HostedToolActivity {
        name: String,
        detail: String,
    },
    /// The provider completed a request on a different service tier.
    ProviderServiceTierFallback {
        requested: crate::model::ServiceTier,
        used: String,
    },
    /// The backend acknowledged a steer inside the current model turn.
    ///
    /// [`RunEvent::SteeringApplied`] still fires when that input crosses into
    /// conversation history at the turn boundary.
    SteeringDelivered {
        id: SteeringId,
    },
    /// The call runs detached: the loop continues and `ToolFinished` for this id
    /// arrives after later `StepStarted` events. Hosts keep the card alive.
    ///
    /// # Next major
    ///
    /// NEXT_MAJOR(rho-sdk): fold ToolDetached into ToolStarted as an execution field.
    ///
    /// This minor cannot add fields to [`RunEvent::ToolStarted`], so detached
    /// execution is a sibling event. Prefer matching through host helpers until
    /// major.
    ToolDetached {
        call_id: ToolCallId,
    },
}

#[cfg(test)]
#[path = "event_tests.rs"]
mod tests;
