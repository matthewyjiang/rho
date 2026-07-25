//! Human-readable compaction tool-block display lines.
//!
//! Compaction is shown like a tool call: a running block while work is in
//! flight, then a finished block with the outcome facts we actually have.

use rho_sdk::ToolCallId;

use super::usage_cost::format_usd;

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

    pub(super) fn reduced(self) -> bool {
        self.removed_tokens() > 0 || self.removed_messages() > 0
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
    pub(super) fn ok(&self) -> bool {
        !matches!(self, Self::Failed { .. })
    }

    pub(super) fn display_lines(&self) -> Vec<String> {
        match self {
            Self::Completed(facts) => completed_display_lines(*facts),
            Self::Unchanged { detail } => unchanged_display_lines(detail.clone()),
            Self::Failed { detail } => failed_display_lines(detail.clone()),
            Self::Cancelled => cancelled_display_lines(),
        }
    }
}

/// Running tool-block lines (layout A).
pub(super) fn running_display_lines() -> Vec<String> {
    vec!["compact".into(), "shrinking context…".into()]
}

/// Finished tool-block lines (layout 1), driven only by available facts.
pub(super) fn completed_display_lines(facts: CompactionDisplayFacts) -> Vec<String> {
    let mut lines = vec!["compact".into()];

    let has_token_signal = facts.previous_tokens > 0 || facts.current_tokens > 0;
    if has_token_signal {
        lines.push(token_summary_line(facts));
    }

    // Always show messages when tokens are missing, or when the message count
    // changed / both sides are non-zero so the reduction is legible.
    if !has_token_signal
        || facts.previous_messages != facts.current_messages
        || facts.previous_messages > 0
    {
        lines.push(message_summary_line(facts));
    }

    if let Some(cost) = facts.cost_usd_micros.filter(|cost| *cost > 0) {
        lines.push(format!("cost {}", format_usd(cost)));
    }

    if !facts.reduced() && lines.len() == 1 {
        lines.push("no reduction".into());
    }

    lines
}

/// Failed compact tool-block lines.
pub(super) fn failed_display_lines(detail: impl Into<String>) -> Vec<String> {
    vec!["compact".into(), "failed".into(), detail.into()]
}

/// Cancelled compact tool-block lines.
pub(super) fn cancelled_display_lines() -> Vec<String> {
    vec!["compact".into(), "cancelled".into()]
}

/// Unchanged / not-enough-history finished block.
pub(super) fn unchanged_display_lines(detail: impl Into<String>) -> Vec<String> {
    vec!["compact".into(), detail.into()]
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

fn format_token_count(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    }
}

#[cfg(test)]
#[path = "compaction_display_tests.rs"]
mod tests;
