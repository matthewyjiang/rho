use super::{thought_summary, ReasoningPhase};
use crate::tui::goal::{format_elapsed, format_elapsed_with, ElapsedPrecision};
use pretty_assertions::assert_eq;
use std::time::Duration;

#[test]
fn formats_compact_progressive_durations() {
    assert_eq!(
        format_elapsed_with(
            Duration::from_millis(0),
            ElapsedPrecision::TenthsUnderMinute
        ),
        "0.0s"
    );
    assert_eq!(
        format_elapsed_with(
            Duration::from_millis(320),
            ElapsedPrecision::TenthsUnderMinute
        ),
        "0.3s"
    );
    assert_eq!(
        format_elapsed_with(
            Duration::from_millis(3_200),
            ElapsedPrecision::TenthsUnderMinute
        ),
        "3.2s"
    );
    assert_eq!(
        format_elapsed_with(Duration::from_secs(45), ElapsedPrecision::TenthsUnderMinute),
        "45.0s"
    );
    assert_eq!(
        format_elapsed_with(Duration::from_secs(65), ElapsedPrecision::TenthsUnderMinute),
        "1m 5s"
    );
    assert_eq!(
        format_elapsed_with(
            Duration::from_secs(125),
            ElapsedPrecision::TenthsUnderMinute
        ),
        "2m 5s"
    );
    assert_eq!(
        format_elapsed_with(
            Duration::from_secs(3_720),
            ElapsedPrecision::TenthsUnderMinute
        ),
        "1h 2m"
    );
    // Whole-second style stays available for other TUI labels.
    assert_eq!(format_elapsed(Duration::from_secs(9)), "9s");
}

#[test]
fn summary_prefixes_thought_for() {
    assert_eq!(
        thought_summary(Duration::from_millis(1_500)),
        "Thought for 1.5s"
    );
}

#[test]
fn finalize_returns_elapsed_only_after_reasoning_deltas() {
    let mut phase = ReasoningPhase::default();
    phase.begin_step(/*show_reasoning*/ false);
    assert!(phase.hidden_placeholder());
    assert!(phase.finalize().is_none());
    assert!(!phase.hidden_placeholder());

    phase.begin_step(/*show_reasoning*/ false);
    phase.on_reasoning_delta(/*show_reasoning*/ false);
    assert!(phase.hidden_placeholder());
    assert!(phase.has_started());
    let elapsed = phase.finalize().expect("timed stretch");
    assert!(elapsed >= Duration::ZERO);
    assert!(!phase.hidden_placeholder());
    assert!(!phase.has_started());
}
