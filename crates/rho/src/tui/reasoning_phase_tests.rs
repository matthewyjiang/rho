use super::ReasoningPhase;
use std::time::Duration;

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
