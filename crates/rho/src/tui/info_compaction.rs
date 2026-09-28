//! Human-readable counterpart of `rho(action="compaction")`.
use crate::{compaction_metrics::CompactionRecord, diagnostics::CompactionDiagnostics};

use super::{format_number, push_optional_number, CommandBlock};

pub(super) fn push_fields(block: &mut CommandBlock, diagnostics: &CompactionDiagnostics) {
    let current = &diagnostics.current;
    if !current.enabled
        && diagnostics.last_idle_check.is_none()
        && diagnostics.last_provider_check.is_none()
        && diagnostics.completed.completed_compactions() == 0
    {
        return;
    }
    block.push_section("Compaction");
    block.push_field("Context", &current.estimate.tokens().to_string());
    block.push_field(
        "Local tokens",
        &current.estimate.estimated_tokens().to_string(),
    );
    push_optional_number(
        block,
        "Provider",
        current.estimate.provider_reported_tokens(),
    );
    push_optional_number(
        block,
        "Request est.",
        current.estimate.provider_request_estimated_tokens(),
    );
    push_optional_number(block, "Window", current.context_window);
    push_optional_number(block, "Threshold", current.threshold_tokens);
    push_optional_number(block, "Target", current.target_tokens);
    push_optional_number(block, "Local target", current.estimated_target_tokens);
    let completed = &diagnostics.completed;
    block.push_field("Completed", &completed.completed_compactions().to_string());
    block.push_field("Removed local", &completed.removed_tokens().to_string());
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
        block.push_field(
            "Last completed",
            &format!("{before} → {after} local tokens ({result})"),
        );
    }
    if let Some(record) = &diagnostics.last_compaction {
        push_last_compaction(block, record);
    }
    if let Some(check) = &diagnostics.last_idle_check {
        block.push_field(
            "Idle check",
            &format!(
                "{:?}, {} tokens",
                check.reason,
                check.context.estimate.tokens()
            ),
        );
    }
    if let Some(check) = &diagnostics.last_provider_check {
        let check = check.decision;
        block.push_field(
            "SDK check",
            &format!(
                "{} tokens, threshold {:?}, skip {:?}, extent {:?}",
                check.estimate().tokens(),
                check.threshold(),
                check.skip_reason(),
                check.extent(),
            ),
        );
    }
}

/// One line for what the latest compactor call was, one for what it cost,
/// and one for what followed it once committed.
fn push_last_compaction(block: &mut CommandBlock, record: &CompactionRecord) {
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
    block.push_field("Last run", &kind);

    let mut cost = vec![format!("{} ms", format_number(record.latency_ms))];
    let tokens = [
        ("in", record.prompt_tokens),
        ("out", record.output_tokens),
        ("cache read", record.cache_read_tokens),
    ];
    cost.extend(
        tokens
            .into_iter()
            .filter_map(|(label, value)| Some(format!("{} {label}", format_number(value?)))),
    );
    if let Some(micros) = record.cost_usd_micros {
        cost.push(format!("${:.4}", micros as f64 / 1_000_000.0));
    }
    if let Some(model) = &record.model {
        cost.push(model.clone());
    }
    block.push_field("Run cost", &cost.join(" · "));

    let mut after = Vec::new();
    if let Some(tokens) = record.next_prompt_tokens {
        after.push(format!("next prompt {}", format_number(tokens)));
    }
    if let Some(reread) = record.reread {
        after.push(format!(
            "{}/{} calls repeated removed reads or commands ({} watched, window {})",
            reread.repeated, reread.tool_calls, reread.tracked, reread.window
        ));
    }
    if !after.is_empty() {
        block.push_field("After run", &after.join(" · "));
    }
}
