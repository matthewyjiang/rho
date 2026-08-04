use super::ReasoningPhase;
use std::time::Duration;

#[test]
fn finalize_returns_elapsed_only_after_reasoning_deltas() {
    let mut phase = ReasoningPhase::default();
    phase.begin_step(/*show_thinking_placeholder*/ true);
    assert!(phase.hidden_placeholder());
    assert!(phase.finalize().is_none());
    assert!(!phase.hidden_placeholder());

    phase.begin_step(/*show_thinking_placeholder*/ true);
    phase.on_reasoning_delta(/*show_thinking_placeholder*/ true);
    assert!(phase.hidden_placeholder());
    assert!(phase.has_started());
    let elapsed = phase.finalize().expect("timed stretch");
    assert!(elapsed >= Duration::ZERO);
    assert!(!phase.hidden_placeholder());
    assert!(!phase.has_started());
}

#[test]
fn begin_step_can_suppress_thinking_placeholder() {
    let mut phase = ReasoningPhase::default();
    phase.begin_step(/*show_thinking_placeholder*/ false);
    assert!(!phase.hidden_placeholder());
    phase.on_reasoning_delta(/*show_thinking_placeholder*/ false);
    assert!(phase.has_started());
    assert!(!phase.hidden_placeholder());
}
