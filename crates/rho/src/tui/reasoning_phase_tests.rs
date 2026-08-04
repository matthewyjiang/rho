use super::ReasoningPhase;
use std::time::Duration;

#[test]
fn finalize_returns_elapsed_only_after_reasoning_deltas() {
    let mut phase = ReasoningPhase::default();
    phase.begin_step();
    assert!(phase.is_open());
    // No deltas yet: close without a timed summary.
    assert!(phase.finalize().is_none());
    assert!(!phase.is_open());

    phase.begin_step();
    phase.on_reasoning_delta();
    assert!(phase.is_open());
    let elapsed = phase.finalize().expect("timed stretch");
    assert!(elapsed >= Duration::ZERO);
    assert!(!phase.is_open());
}

#[test]
fn reset_closes_open_stretch_without_elapsed() {
    let mut phase = ReasoningPhase::default();
    phase.begin_step();
    phase.on_reasoning_delta();
    assert!(phase.is_open());
    phase.reset();
    assert!(!phase.is_open());
    // Reset discards the timer; a later finalize must not invent elapsed.
    assert!(phase.finalize().is_none());
}
