use crate::{CompactionPolicy, CompactionThreshold, ContextEstimate};

/// Why an automatic compaction checkpoint did not request compaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub enum CompactionSkipReason {
    PolicyDisabled,
    BelowThreshold,
    PendingAsyncTools,
}

/// The latest automatic policy evaluation. A due decision is not a success report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CompactionDecision {
    estimate: ContextEstimate,
    threshold: Option<CompactionThreshold>,
    skip_reason: Option<CompactionSkipReason>,
}

impl CompactionDecision {
    pub(crate) fn evaluate(
        policy: Option<&CompactionPolicy>,
        message_count: usize,
        estimate: ContextEstimate,
    ) -> Self {
        let skip_reason = match policy {
            None => Some(CompactionSkipReason::PolicyDisabled),
            Some(policy) if !policy.should_compact(message_count, estimate.tokens()) => {
                Some(CompactionSkipReason::BelowThreshold)
            }
            Some(_) => None,
        };
        Self {
            estimate,
            threshold: policy.map(CompactionPolicy::threshold),
            skip_reason,
        }
    }

    pub(crate) fn with_pending_tools(mut self) -> Self {
        if self.threshold.is_some() {
            self.skip_reason = Some(CompactionSkipReason::PendingAsyncTools);
        }
        self
    }

    pub const fn estimate(self) -> ContextEstimate {
        self.estimate
    }

    pub const fn threshold(self) -> Option<CompactionThreshold> {
        self.threshold
    }

    /// `None` means the policy requested compaction, which may still fail or cancel.
    pub const fn skip_reason(self) -> Option<CompactionSkipReason> {
        self.skip_reason
    }
}
