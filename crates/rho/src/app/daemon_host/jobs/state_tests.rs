use pretty_assertions::assert_eq;

use super::{JobState, TransitionRefused};

const ALL: [JobState; 7] = [
    JobState::Idle,
    JobState::Running,
    JobState::WaitingForInput,
    JobState::Completed,
    JobState::Failed,
    JobState::Cancelled,
    JobState::Interrupted,
];

// Covers: every (from, to) pair a job state can be asked for: a prompt
// starts a turn from idle or any ended state, a running turn toggles
// waiting_for_input and ends from either, and everything else is refused
// with the pair named, including a second prompt on a running thread
// (running -> running), a turn ending twice, and a turn ending before it
// started.
// Owner: daemon job state machine.
#[test]
fn transitions_allow_exactly_the_turn_lifecycle() {
    use JobState::{Cancelled, Completed, Failed, Idle, Interrupted, Running, WaitingForInput};
    let allowed = [
        (Idle, Running),
        (Completed, Running),
        (Failed, Running),
        (Cancelled, Running),
        (Interrupted, Running),
        (Running, WaitingForInput),
        (WaitingForInput, Running),
        (Running, Completed),
        (Running, Failed),
        (Running, Cancelled),
        (Running, Interrupted),
        (WaitingForInput, Completed),
        (WaitingForInput, Failed),
        (WaitingForInput, Cancelled),
        (WaitingForInput, Interrupted),
    ];
    for from in ALL {
        for to in ALL {
            let expected = if allowed.contains(&(from, to)) {
                Ok(to)
            } else {
                Err(TransitionRefused { from, to })
            };
            assert_eq!(from.transition(to), expected, "{from} -> {to}");
        }
    }
}
