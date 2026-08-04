use super::ReasoningPhase;
use std::time::Duration;

#[test]
fn finalize_returns_elapsed_only_after_reasoning_deltas() {
    let mut phase = ReasoningPhase::default();
    phase.begin_step();
    assert!(phase.is_open());
    assert!(!phase.has_started());
    assert!(phase.finalize().is_none());
    assert!(!phase.is_open());

    phase.begin_step();
    phase.on_reasoning_delta();
    assert!(phase.is_open());
    assert!(phase.has_started());
    let elapsed = phase.finalize().expect("timed stretch");
    assert!(elapsed >= Duration::ZERO);
    assert!(!phase.is_open());
    assert!(!phase.has_started());
}

#[test]
fn reset_closes_open_stretch_without_elapsed() {
    let mut phase = ReasoningPhase::default();
    phase.begin_step();
    phase.on_reasoning_delta();
    assert!(phase.is_open());
    assert!(phase.has_started());
    phase.reset();
    assert!(!phase.is_open());
    assert!(!phase.has_started());
}
