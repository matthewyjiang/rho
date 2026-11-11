//! The `/info` Compaction section: the human-readable counterpart of
//! `rho(action="compaction")`.
//!
//! Built where the session runs and shown as rows, so the TUI reads the
//! same section from its own runtime and from a daemon session
//! (`SessionReport::Compaction`).

use serde::{Deserialize, Serialize};

use crate::{
    compaction_metrics::CompactionRecord, diagnostics::CompactionDiagnostics,
    presentation::number::grouped,
};

/// One labelled value of the section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompactionRow {
    pub(crate) label: String,
    pub(crate) value: String,
}

/// The section's rows in display order; none when the session has nothing
/// to say about compaction.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompactionReport {
    pub(crate) rows: Vec<CompactionRow>,
}

impl CompactionReport {
    pub(crate) fn new(diagnostics: &CompactionDiagnostics) -> Self {
        let mut report = Self::default();
        let current = &diagnostics.current;
        if !current.enabled
            && diagnostics.last_idle_check.is_none()
            && diagnostics.last_provider_check.is_none()
            && diagnostics.completed.completed_compactions() == 0
        {
            return report;
        }
        report.push("Context", current.estimate.tokens().to_string());
        report.push(
            "Local tokens",
            current.estimate.estimated_tokens().to_string(),
        );
        report.push_number("Provider", current.estimate.provider_reported_tokens());
        report.push_number(
            "Request est.",
            current.estimate.provider_request_estimated_tokens(),
        );
        report.push_number("Window", current.context_window);
        report.push_number("Threshold", current.threshold_tokens);
        report.push_number("Target", current.target_tokens);
        report.push_number("Local target", current.estimated_target_tokens);
        let completed = &diagnostics.completed;
        report.push("Completed", completed.completed_compactions().to_string());
        report.push("Removed local", completed.removed_tokens().to_string());
        if let (Some(before), Some(after)) = (
            completed.last_previous_tokens(),
            completed.last_current_tokens(),
        ) {
            let result = if after < before {
                "reduced"
            } else if after == before {
                "unchanged"
            } else {
                "increased"
            };
            report.push(
                "Last completed",
                format!("{before} → {after} local tokens ({result})"),
            );
        }
        if let Some(record) = &diagnostics.last_compaction {
            report.push_last_compaction(record);
        }
        if let Some(check) = &diagnostics.last_idle_check {
            report.push(
                "Idle check",
                format!(
                    "{:?}, {} tokens",
                    check.reason,
                    check.context.estimate.tokens()
                ),
            );
        }
        if let Some(check) = &diagnostics.last_provider_check {
            let check = check.decision;
            report.push(
                "SDK check",
                format!(
                    "{} tokens, threshold {:?}, skip {:?}, extent {:?}",
                    check.estimate().tokens(),
                    check.threshold(),
                    check.skip_reason(),
                    check.extent(),
                ),
            );
        }
        report
    }

    fn push(&mut self, label: &str, value: String) {
        self.rows.push(CompactionRow {
            label: label.to_owned(),
            value,
        });
    }

    fn push_number(&mut self, label: &str, value: Option<u64>) {
        if let Some(value) = value {
            self.push(label, grouped(value));
        }
    }

    /// One row for what the latest compactor call was, one for what it cost,
    /// and one for what followed it once committed.
    fn push_last_compaction(&mut self, record: &CompactionRecord) {
        let mut kind = format!("{} {}", record.trigger.label(), record.outcome.label());
        if let Some(tier) = record.tier {
            kind.push_str(&format!(", {}", tier.label()));
        }
        if let Some(path) = record.request_path {
            kind.push_str(&format!(" via {}", path.label()));
        }
        if record.elided_tool_results > 0 {
            kind.push_str(&format!(
                ", {} tool results elided",
                record.elided_tool_results
            ));
        }
        self.push("Last run", kind);

        let mut cost = vec![format!("{} ms", grouped(record.latency_ms))];
        let tokens = [
            ("in", record.prompt_tokens),
            ("out", record.output_tokens),
            ("cache read", record.cache_read_tokens),
        ];
        cost.extend(
            tokens
                .into_iter()
                .filter_map(|(label, value)| Some(format!("{} {label}", grouped(value?)))),
        );
        if let Some(micros) = record.cost_usd_micros {
            cost.push(format!("${:.4}", micros as f64 / 1_000_000.0));
        }
        if let Some(model) = &record.model {
            cost.push(model.clone());
        }
        self.push("Run cost", cost.join(" · "));

        let mut after = Vec::new();
        if let Some(tokens) = record.next_prompt_tokens {
            after.push(format!("next prompt {}", grouped(tokens)));
        }
        if let Some(reread) = record.reread {
            after.push(format!(
                "{}/{} calls repeated removed reads or commands ({} watched, window {})",
                reread.repeated, reread.tool_calls, reread.tracked, reread.window
            ));
        }
        if !after.is_empty() {
            self.push("After run", after.join(" · "));
        }
    }
}
