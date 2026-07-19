use crate::{
    model::{ContentBlock, ModelUsage, ToolCall},
    tool::{ToolErrorKind, ToolMetadata, ToolOutput, ToolProgress},
    Revision, RunId, SteeringId, ToolCallId,
};

/// Legacy provider activity kind emitted when a malformed response is retried.
///
/// New hosts should use [`RunEvent::ProviderStreamReset`]. This activity is
/// retained so existing hosts still discard malformed response attempts.
pub const PROVIDER_ACTIVITY_INVALID_RESPONSE_RETRY: &str = "invalid_response_retry";
/// Provider activity kind emitted when a physical provider request is retried.
pub const PROVIDER_ACTIVITY_REQUEST_RETRY: &str = "provider_request_retry";
/// Provider activity kind emitted for provider-native web searches.
pub const PROVIDER_ACTIVITY_WEB_SEARCH: &str = "web_search";

/// Why the current provider attempt was abandoned before a fresh request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProviderStreamResetReason {
    /// The provider returned a malformed normalized assistant response.
    InvalidResponse,
    /// The provider request failed with a retryable error.
    RetryableFailure(crate::ProviderErrorKind),
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
}
