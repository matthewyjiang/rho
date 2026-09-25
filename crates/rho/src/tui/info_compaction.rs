//! Human-readable counterpart of `rho(action="compaction")`.
use crate::diagnostics::CompactionDiagnostics;

use super::{push_optional_number, CommandBlock};

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
    if let Some(report) = &diagnostics.last_tier {
        block.push_field(
            "Last tier",
            &format!(
                "{:?}, {} tool results elided",
                report.tier, report.elided_tool_results
            ),
        );
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
                "{} tokens, threshold {:?}, skip {:?}",
                check.estimate().tokens(),
                check.threshold(),
                check.skip_reason(),
            ),
        );
    }
}
