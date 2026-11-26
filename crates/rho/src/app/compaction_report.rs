//! The `/info` Compaction section: the human-readable counterpart of
//! `rho(action="compaction")`.
//!
//! Built where the session runs and shown as rows, so the TUI reads the
//! same section from its own runtime and from a daemon session
//! (`SessionReport::Compaction`). [`session_compaction_report`] builds the
//! typed report a daemon session answers, and the rows are read from it.

use rho_protocol::{
    CompactionCheck, CompactionContextSize, CompactionReread, CompactionRun, CompactionSize,
    CompletedCompactions, ModelRef, SessionCompactionReport,
};
use rho_sdk::{CompactionExtent, CompactionSkipReason, CompactionThreshold};
use serde::{Deserialize, Serialize};

use crate::{
    compaction_metrics::{
        CompactionRecord, CompactionRunOutcome, CompactionTier, CompactionTriggerKind,
        SummaryRequestPath,
    },
    diagnostics::CompactionDiagnostics,
    presentation::number::grouped,
};

/// One labelled value of the section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompactionRow {
    pub(crate) label: String,
    pub(crate) value: String,
}

/// The section's rows in display order, read from the typed report; none
/// when the session has nothing to say about compaction.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompactionReport {
    pub(crate) rows: Vec<CompactionRow>,
}

impl From<&SessionCompactionReport> for CompactionReport {
    fn from(typed: &SessionCompactionReport) -> Self {
        let mut report = Self::default();
        let completed = &typed.completed;
        if !typed.auto_compact && typed.last_check.is_none() && completed.count == 0 {
            return report;
        }
        let context = &typed.context;
        report.push("Context", context.tokens.to_string());
        report.push("Local tokens", context.local_tokens.to_string());
        report.push_number("Provider", context.provider_reported_tokens);
        report.push_number("Request est.", context.provider_request_local_tokens);
        report.push_number("Window", typed.window_tokens);
        report.push_number("Threshold", typed.threshold_tokens);
        report.push_number("Target", typed.target_tokens);
        report.push_number("Local target", typed.local_target_tokens);
        report.push("Completed", completed.count.to_string());
        report.push("Removed local", completed.removed_local_tokens.to_string());
        if let Some(CompactionSize {
            before_local_tokens: before,
            after_local_tokens: after,
        }) = completed.last
        {
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
        if let Some(run) = &typed.last_run {
            report.push_last_run(run);
        }
        if let Some(check) = &typed.last_check {
            report.push(
                "SDK check",
                format!(
                    "{} tokens, threshold {:?}, skip {:?}, extent {:?}",
                    check.context_tokens, check.threshold, check.skipped, check.extent,
                ),
            );
        }
        report
    }
}

impl CompactionReport {
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
    fn push_last_run(&mut self, run: &CompactionRun) {
        let mut kind = format!("{} {}", wire_name(run.trigger), wire_name(run.outcome));
        if let Some(tier) = run.tier {
            kind.push_str(&format!(", {}", wire_name(tier)));
        }
        if let Some(path) = run.summary_request {
            kind.push_str(&format!(" via {}", wire_name(path)));
        }
        if run.elided_tool_results > 0 {
            kind.push_str(&format!(
                ", {} tool results elided",
                run.elided_tool_results
            ));
        }
        self.push("Last run", kind);

        let mut cost = vec![format!("{} ms", grouped(run.latency_ms))];
        let tokens = [
            ("in", run.prompt_tokens),
            ("out", run.output_tokens),
            ("cache read", run.cache_read_tokens),
        ];
        cost.extend(
            tokens
                .into_iter()
                .filter_map(|(label, value)| Some(format!("{} {label}", grouped(value?)))),
        );
        if let Some(micros) = run.cost_usd_micros {
            cost.push(format!("${:.4}", micros as f64 / 1_000_000.0));
        }
        if let Some(model) = &run.model {
            cost.push(format!("{}/{}", model.provider, model.model));
        }
        self.push("Run cost", cost.join(" · "));

        let mut after = Vec::new();
        if let Some(tokens) = run.next_prompt_tokens {
            after.push(format!("next prompt {}", grouped(tokens)));
        }
        if let Some(reread) = run.reread {
            after.push(format!(
                "{}/{} calls repeated removed reads or commands ({} watched, window {})",
                reread.repeated, reread.tool_calls, reread.tracked, reread.window_tool_calls
            ));
        }
        if !after.is_empty() {
            self.push("After run", after.join(" · "));
        }
    }
}

/// The snake_case name a protocol enum value has on the wire.
fn wire_name(value: impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unknown".to_owned(),
    }
}

/// The typed report of `diagnostics`, which a daemon session answers
/// (`SessionReport::Compaction`).
pub(crate) fn session_compaction_report(
    diagnostics: &CompactionDiagnostics,
) -> SessionCompactionReport {
    let current = &diagnostics.current;
    let completed = &diagnostics.completed;
    SessionCompactionReport {
        auto_compact: current.enabled,
        context: CompactionContextSize {
            tokens: current.estimate.tokens(),
            local_tokens: current.estimate.estimated_tokens(),
            provider_reported_tokens: current.estimate.provider_reported_tokens(),
            provider_request_local_tokens: current.estimate.provider_request_estimated_tokens(),
        },
        window_tokens: current.context_window,
        threshold_tokens: current.threshold_tokens,
        target_tokens: current.target_tokens,
        local_target_tokens: current.estimated_target_tokens,
        completed: CompletedCompactions {
            count: completed.completed_compactions(),
            removed_local_tokens: completed.removed_tokens(),
            last: completed
                .last_previous_tokens()
                .zip(completed.last_current_tokens())
                .map(|(before, after)| CompactionSize {
                    before_local_tokens: before,
                    after_local_tokens: after,
                }),
        },
        last_run: diagnostics.last_compaction.as_ref().map(compaction_run),
        last_check: diagnostics.last_provider_check.as_ref().map(|check| {
            let decision = check.decision;
            CompactionCheck {
                context_tokens: decision.estimate().tokens(),
                threshold: decision.threshold().map(|threshold| match threshold {
                    CompactionThreshold::Messages(messages) => {
                        rho_protocol::CompactionThreshold::Messages {
                            messages: messages.get() as u64,
                        }
                    }
                    CompactionThreshold::ContextTokens(tokens) => {
                        rho_protocol::CompactionThreshold::ContextTokens {
                            tokens: tokens.get(),
                        }
                    }
                    _ => rho_protocol::CompactionThreshold::Unknown,
                }),
                skipped: decision.skip_reason().map(|reason| match reason {
                    CompactionSkipReason::PolicyDisabled => {
                        rho_protocol::CompactionSkipReason::PolicyDisabled
                    }
                    CompactionSkipReason::BelowThreshold => {
                        rho_protocol::CompactionSkipReason::BelowThreshold
                    }
                    CompactionSkipReason::PendingAsyncTools => {
                        rho_protocol::CompactionSkipReason::PendingAsyncTools
                    }
                    _ => rho_protocol::CompactionSkipReason::Unknown,
                }),
                extent: match decision.extent() {
                    CompactionExtent::History => rho_protocol::CompactionExtent::History,
                    CompactionExtent::BeforePendingAsyncTools => {
                        rho_protocol::CompactionExtent::BeforePendingAsyncTools
                    }
                    _ => rho_protocol::CompactionExtent::Unknown,
                },
            }
        }),
    }
}

fn compaction_run(record: &CompactionRecord) -> CompactionRun {
    CompactionRun {
        trigger: match record.trigger {
            CompactionTriggerKind::Automatic => rho_protocol::CompactionRunTrigger::Automatic,
            CompactionTriggerKind::Manual => rho_protocol::CompactionRunTrigger::Manual,
            CompactionTriggerKind::ContextOverflow => {
                rho_protocol::CompactionRunTrigger::ContextOverflow
            }
            CompactionTriggerKind::Other => rho_protocol::CompactionRunTrigger::Unknown,
        },
        outcome: match record.outcome {
            CompactionRunOutcome::Completed => rho_protocol::CompactionRunOutcome::Completed,
            CompactionRunOutcome::Failed => rho_protocol::CompactionRunOutcome::Failed,
            CompactionRunOutcome::Cancelled => rho_protocol::CompactionRunOutcome::Cancelled,
        },
        tier: record.tier.map(|tier| match tier {
            CompactionTier::Elision => rho_protocol::CompactionTier::Elision,
            CompactionTier::Native => rho_protocol::CompactionTier::Native,
            CompactionTier::TextSummary => rho_protocol::CompactionTier::TextSummary,
            CompactionTier::Unchanged => rho_protocol::CompactionTier::Unchanged,
        }),
        summary_request: record.request_path.map(|path| match path {
            SummaryRequestPath::SessionHistory => rho_protocol::SummaryRequestPath::SessionHistory,
            SummaryRequestPath::Transcript => rho_protocol::SummaryRequestPath::Transcript,
            SummaryRequestPath::Summarizer => rho_protocol::SummaryRequestPath::Summarizer,
        }),
        // The record keeps a `provider/model` reference; providers split it
        // at its first `/`.
        model: record
            .model
            .as_deref()
            .and_then(|reference| reference.split_once('/'))
            .map(|(provider, model)| ModelRef {
                provider: provider.to_owned(),
                model: model.to_owned(),
            }),
        elided_tool_results: record.elided_tool_results as u64,
        context_tokens: record.context_tokens,
        prompt_tokens: record.prompt_tokens,
        output_tokens: record.output_tokens,
        cache_read_tokens: record.cache_read_tokens,
        cost_usd_micros: record.cost_usd_micros,
        latency_ms: record.latency_ms,
        next_prompt_tokens: record.next_prompt_tokens,
        reread: record.reread.map(|reread| CompactionReread {
            window_tool_calls: reread.window.into(),
            tool_calls: reread.tool_calls.into(),
            repeated: reread.repeated.into(),
            tracked: reread.tracked.into(),
        }),
    }
}
