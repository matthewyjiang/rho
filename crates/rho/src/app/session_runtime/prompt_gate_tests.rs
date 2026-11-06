use super::PromptGate;

// Covers: a cancel during prompt start must still cancel the run
// Owner: session runtime prompt gate
#[test]
fn cancel_during_start_marks_the_gate() {
    let gate = PromptGate::new();
    let starting = gate.begin();
    gate.cancel();
    assert!(
        starting.is_cancelled(),
        "command expansion must see the cancel"
    );
    let token = rho_sdk::CancellationToken::new();
    gate.activate(token.clone());
    assert!(token.is_cancelled());
}

// Covers: a cancel aimed at a finished prompt must not cancel the next one
// Owner: session runtime prompt gate
#[test]
fn cancel_before_the_next_prompt_does_not_leak_into_it() {
    let gate = PromptGate::new();
    gate.begin();
    gate.finish();
    gate.cancel();
    gate.begin();
    let token = rho_sdk::CancellationToken::new();
    gate.activate(token.clone());
    assert!(!token.is_cancelled());
}

// Covers: a cancel that lands between two turns of one prompt (plan approval
// runs the implementation turn) must cancel the second turn too
// Owner: session runtime prompt gate
#[test]
fn cancel_between_turns_of_one_prompt_cancels_the_next_turn() {
    let gate = PromptGate::new();
    gate.begin();
    let first = rho_sdk::CancellationToken::new();
    gate.activate(first.clone());
    gate.cancel();
    let second = rho_sdk::CancellationToken::new();
    gate.activate(second.clone());
    assert!(second.is_cancelled());
}
