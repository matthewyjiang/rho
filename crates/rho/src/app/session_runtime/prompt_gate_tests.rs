use pretty_assertions::assert_eq;

use super::PromptGate;

// Covers: a cancel during prompt start must still cancel the run
// Owner: session runtime prompt gate
#[test]
fn cancel_during_start_marks_the_gate() {
    let gate = PromptGate::new();
    let (starting, generation) = gate.begin();
    gate.cancel();
    assert!(
        starting.is_cancelled(),
        "command expansion must see the cancel"
    );
    let token = rho_sdk::CancellationToken::new();
    gate.activate(generation, token.clone());
    assert!(token.is_cancelled());
}

// Covers: a cancel aimed at a finished prompt must not cancel the next one
// Owner: session runtime prompt gate
#[test]
fn cancel_before_the_next_prompt_does_not_leak_into_it() {
    let gate = PromptGate::new();
    let (_, first) = gate.begin();
    gate.finish(first);
    gate.cancel();
    let (_, second) = gate.begin();
    let token = rho_sdk::CancellationToken::new();
    gate.activate(second, token.clone());
    assert!(!token.is_cancelled());
}

// Covers: a cancel that lands between two turns of one prompt (plan approval
// runs the implementation turn) must cancel the second turn too
// Owner: session runtime prompt gate
#[test]
fn cancel_between_turns_of_one_prompt_cancels_the_next_turn() {
    let gate = PromptGate::new();
    let (_, generation) = gate.begin();
    let first = rho_sdk::CancellationToken::new();
    gate.activate(generation, first.clone());
    gate.cancel();
    let second = rho_sdk::CancellationToken::new();
    gate.activate(generation, second.clone());
    assert!(second.is_cancelled());
}

// Covers: a prompt's scope dropped after the next prompt began must neither
// end the newer prompt (a cancel must still reach it) nor arm a run for it,
// and reads its own prompt as over
// Owner: session runtime prompt gate
#[test]
fn a_stale_prompt_never_acts_for_the_newer_one() {
    let gate = PromptGate::new();
    let (_, stale) = gate.begin();
    let (_, current) = gate.begin();
    let current_run = rho_sdk::CancellationToken::new();
    gate.activate(current, current_run.clone());
    let stale_run = rho_sdk::CancellationToken::new();
    gate.activate(stale, stale_run.clone());
    gate.finish(stale);
    gate.cancel();
    assert_eq!(
        (
            current_run.is_cancelled(),
            stale_run.is_cancelled(),
            gate.is_cancelled(stale),
        ),
        (true, true, true)
    );
}
