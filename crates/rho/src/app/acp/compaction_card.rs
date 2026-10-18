//! ACP `compaction_update` cards, for hosts that advertise
//! `clientCapabilities.session.compaction`.
//!
//! A card opens `in_progress` and ends with exactly one terminal update,
//! which consumes it. Each compaction gets a fresh random ID: hosts such as
//! T3 Code keep their transcript across reconnects, so an ID that restarts
//! per process would patch an earlier card in place.

use agent_client_protocol::schema::v1::{
    CompactionId, CompactionStatus, CompactionUpdate, ContentBlock, SessionUpdate,
};

/// Error for a compaction the run ended before it reported a result.
pub(super) const UNFINISHED: &str = "compaction ended without a result";

/// One compaction the host shows as a card.
#[derive(Debug)]
pub(super) struct CompactionCard {
    id: CompactionId,
}

impl CompactionCard {
    pub(super) fn new() -> Self {
        Self {
            id: CompactionId::new(format!("compaction-{}", uuid::Uuid::new_v4())),
        }
    }

    pub(super) fn in_progress(&self) -> SessionUpdate {
        self.update(CompactionStatus::InProgress)
    }

    /// The finished card, summarized with the before and after counts.
    pub(super) fn completed(self, outcome: &rho_sdk::CompactionOutcome) -> SessionUpdate {
        SessionUpdate::CompactionUpdate(
            CompactionUpdate::new(self.id, CompactionStatus::Completed)
                .summary(vec![ContentBlock::from(outcome_summary(outcome))]),
        )
    }

    pub(super) fn failed(self, error: impl Into<String>) -> SessionUpdate {
        SessionUpdate::CompactionUpdate(
            CompactionUpdate::new(self.id, CompactionStatus::Failed).error(error.into()),
        )
    }

    pub(super) fn cancelled(self) -> SessionUpdate {
        self.update(CompactionStatus::Cancelled)
    }

    fn update(&self, status: CompactionStatus) -> SessionUpdate {
        SessionUpdate::CompactionUpdate(CompactionUpdate::new(self.id.clone(), status))
    }
}

/// What a finished compaction did, in the words `/compact` replies with.
pub(super) fn outcome_summary(outcome: &rho_sdk::CompactionOutcome) -> String {
    if crate::compaction::outcome_reduced_context(outcome) {
        format!(
            "context compacted: {} → {} messages, {} → {} tokens",
            outcome.previous_messages(),
            outcome.current_messages(),
            outcome.previous_tokens(),
            outcome.current_tokens(),
        )
    } else {
        "context not compacted: compaction ran but did not reduce context".into()
    }
}
