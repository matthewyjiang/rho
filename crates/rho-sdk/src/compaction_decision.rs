use crate::{CompactionPolicy, CompactionThreshold, ContextEstimate};

/// Why an automatic compaction checkpoint did not request compaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub enum CompactionSkipReason {
    PolicyDisabled,
    BelowThreshold,
    PendingAsyncTools,
}

/// How much history an automatic compaction checkpoint may rewrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub enum CompactionExtent {
    /// All history, except fresh completion input kept for the next request.
    History,
    /// Only the prefix before the earliest running async tool call. That call
    /// and everything after it stay verbatim so late results still pair.
    BeforePendingAsyncTools,
}

/// The latest automatic policy evaluation. A due decision is not a success report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CompactionDecision {
    estimate: ContextEstimate,
    threshold: Option<CompactionThreshold>,
    skip_reason: Option<CompactionSkipReason>,
    extent: CompactionExtent,
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
            extent: CompactionExtent::History,
        }
    }

    /// Running async tool calls leave nothing new to compact before them, so a
    /// due compaction is skipped.
    pub(crate) fn blocked_by_pending_tools(mut self) -> Self {
        if self.skip_reason.is_none() {
            self.skip_reason = Some(CompactionSkipReason::PendingAsyncTools);
        }
        self
    }

    pub(crate) fn with_extent(mut self, extent: CompactionExtent) -> Self {
        self.extent = extent;
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

    /// History this checkpoint would rewrite when compaction is due.
    pub const fn extent(self) -> CompactionExtent {
        self.extent
    }
}
