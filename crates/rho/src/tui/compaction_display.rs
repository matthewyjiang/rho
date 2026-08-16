//! Shared compact card and view events.
//!
//! Main-loop compact (`/compact`, pre-turn auto-compact) and in-run auto-compact
//! both show this card. They differ only in who owns the session: idle work vs
//! the live turn.

use rho_sdk::ToolCallId;
use rho_tools::tool_card::{ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};

use super::usage_cost::{format_token_count, format_usd};

/// Stable synthetic call id so live compaction can occupy the tool-call batch.
pub(super) fn compaction_call_id() -> ToolCallId {
    ToolCallId::from_string("rho:compact").expect("static compaction call id")
}

/// Facts available after compaction completes. All fields come from
/// [`rho_sdk::CompactionOutcome`] so the layout stays provider-neutral.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CompactionDisplayFacts {
    pub(super) previous_messages: usize,
    pub(super) current_messages: usize,
    pub(super) previous_tokens: u64,
    pub(super) current_tokens: u64,
    pub(super) cost_usd_micros: Option<u64>,
}

impl CompactionDisplayFacts {
    pub(super) fn from_outcome(outcome: &rho_sdk::CompactionOutcome) -> Self {
        Self {
            previous_messages: outcome.previous_messages(),
            current_messages: outcome.current_messages(),
            previous_tokens: outcome.previous_tokens(),
            current_tokens: outcome.current_tokens(),
            cost_usd_micros: outcome.cost_usd_micros(),
        }
    }

    pub(super) fn removed_tokens(self) -> u64 {
        self.previous_tokens.saturating_sub(self.current_tokens)
    }

    pub(super) fn removed_messages(self) -> usize {
        self.previous_messages.saturating_sub(self.current_messages)
    }
}

/// Terminal presentation state for a compaction tool block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CompactionUiOutcome {
    Completed(CompactionDisplayFacts),
    Unchanged { detail: String },
    Failed { detail: String },
    Cancelled,
}

impl CompactionUiOutcome {
    pub(super) fn from_sdk_outcome(outcome: &rho_sdk::CompactionOutcome) -> Self {
        if outcome.reduced_context() {
            Self::Completed(CompactionDisplayFacts::from_outcome(outcome))
        } else {
            Self::unchanged()
        }
    }

    pub(super) fn unchanged() -> Self {
        Self::Unchanged {
            detail:
                "not enough conversation history to compact, or the model context window is unknown"
                    .into(),
        }
    }

    pub(super) fn failed(detail: impl Into<String>) -> Self {
        Self::Failed {
            detail: detail.into(),
        }
    }

    pub(super) fn card(&self) -> ToolCard {
        match self {
            Self::Completed(facts) => completed_card(*facts),
            Self::Unchanged { detail } => unchanged_card(detail.clone()),
            Self::Failed { detail } => failed_card(detail.clone()),
            Self::Cancelled => cancelled_card(),
        }
    }

    pub(super) fn status_label(&self) -> &'static str {
        match self {
            Self::Completed(_) => "context compacted",
            Self::Unchanged { .. } => "context not compacted",
            Self::Failed { .. } => "context compaction failed",
            Self::Cancelled => "context compaction cancelled",
        }
    }
}

/// Running compaction card.
pub(super) fn running_card() -> ToolCard {
    ToolCard::new(
        ToolStatus::Running,
        ToolFamily::Default,
        ToolHeader::call("compact", None),
    )
    .with_facts(vec![ToolFact::Meta {
        text: "shrinking context…".into(),
    }])
}

/// Finished compaction card driven only by available facts.
pub(super) fn completed_card(facts: CompactionDisplayFacts) -> ToolCard {
    let mut card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Default,
        ToolHeader::call("compact", None),
    );

    let has_token_signal = facts.previous_tokens > 0 || facts.current_tokens > 0;
    if has_token_signal {
        card.push_fact(ToolFact::Meta {
            text: token_summary_line(facts),
        });
    }

    // Always show messages when tokens are missing, or when the message count
    // changed / both sides are non-zero so the reduction is legible.
    if !has_token_signal
        || facts.previous_messages != facts.current_messages
        || facts.previous_messages > 0
    {
        card.push_fact(ToolFact::Meta {
            text: message_summary_line(facts),
        });
    }

    if let Some(cost) = facts.cost_usd_micros.filter(|cost| *cost > 0) {
        card.push_fact(ToolFact::Meta {
            text: format!("cost {}", format_usd(cost)),
        });
    }

    card
}

/// Failed compact card.
pub(super) fn failed_card(detail: impl Into<String>) -> ToolCard {
    ToolCard::new(
        ToolStatus::Error,
        ToolFamily::Default,
        ToolHeader::call("compact", None),
    )
    .with_facts(vec![
        ToolFact::Meta {
            text: "failed".into(),
        },
        ToolFact::Error {
            text: detail.into(),
        },
    ])
}

/// Cancelled compact card.
pub(super) fn cancelled_card() -> ToolCard {
    ToolCard::new(
        ToolStatus::Interrupted,
        ToolFamily::Default,
        ToolHeader::call("compact", None),
    )
    .with_facts(vec![ToolFact::Meta {
        text: "cancelled".into(),
    }])
}

/// Unchanged / not-enough-history finished card.
pub(super) fn unchanged_card(detail: impl Into<String>) -> ToolCard {
    ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Default,
        ToolHeader::call("compact", None),
    )
    .with_facts(vec![ToolFact::Meta {
        text: detail.into(),
    }])
}

fn token_summary_line(facts: CompactionDisplayFacts) -> String {
    let mut line = format!(
        "{} → {} tokens",
        format_token_count(facts.previous_tokens),
        format_token_count(facts.current_tokens),
    );
    let removed = facts.removed_tokens();
    if removed > 0 {
        let percent = reduction_percent(facts.previous_tokens, removed);
        line.push_str(&format!(
            "  (−{} · {percent}%)",
            format_token_count(removed)
        ));
    } else if facts.previous_tokens == facts.current_tokens && facts.previous_tokens > 0 {
        line.push_str("  (no change)");
    }
    line
}

fn message_summary_line(facts: CompactionDisplayFacts) -> String {
    let mut line = format!(
        "{} → {} messages",
        facts.previous_messages, facts.current_messages
    );
    let removed = facts.removed_messages();
    if removed > 0 {
        line.push_str(&format!("  (−{removed})"));
    }
    line
}

fn reduction_percent(previous: u64, removed: u64) -> u64 {
    if previous == 0 {
        return 0;
    }
    ((removed as f64 * 100.0) / previous as f64).round() as u64
}
