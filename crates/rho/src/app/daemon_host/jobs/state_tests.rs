use pretty_assertions::assert_eq;

use super::{JobEvent, JobState, TransitionRefused, TurnEnd};

fn failed() -> TurnEnd {
    TurnEnd::Failed("provider refused the request".into())
}

fn all_states() -> [JobState; 8] {
    [
        JobState::Idle,
        JobState::Queued,
        JobState::Running,
        JobState::WaitingForInput,
        JobState::Ended(TurnEnd::Completed),
        JobState::Ended(TurnEnd::Cancelled),
        JobState::Ended(TurnEnd::Interrupted),
        JobState::Ended(failed()),
    ]
}

fn all_events() -> [JobEvent; 9] {
    [
        JobEvent::TurnStarted,
        JobEvent::Admitted,
        JobEvent::InputOpened,
        JobEvent::InputSettled,
        JobEvent::TurnEnded(TurnEnd::Completed),
        JobEvent::TurnEnded(TurnEnd::Cancelled),
        JobEvent::TurnEnded(TurnEnd::Interrupted),
        JobEvent::TurnEnded(failed()),
        JobEvent::ConversationMoved,
    ]
}

// Covers: every (state, event) pair: a prompt starts a turn from idle or
// any ended state, and capacity starts a queued run's, which nothing else
// takes, so a run admitted after its job ended cannot revive it; a running turn opens
// and settles input, and ends, with how it ended, from either, as a queued
// run ends when it is stopped before it starts; a conversation that moves
// rests an idle or ended job idle; everything else is refused
// with the pair named, including a second prompt on a running session, a
// turn ending twice, and a session turn ending before it started.
// Owner: daemon job state machine.
#[test]
fn events_move_a_job_through_exactly_the_turn_lifecycle() {
    use JobEvent::{
        Admitted, ConversationMoved, InputOpened, InputSettled, TurnEnded, TurnStarted,
    };
    use JobState::{Ended, Idle, Queued, Running, WaitingForInput};
    let mut allowed = vec![
        (Idle, TurnStarted, Running),
        (Queued, Admitted, Running),
        (Running, InputOpened, WaitingForInput),
        (WaitingForInput, InputSettled, Running),
        (Idle, ConversationMoved, Idle),
    ];
    for end in [
        TurnEnd::Completed,
        TurnEnd::Cancelled,
        TurnEnd::Interrupted,
        failed(),
    ] {
        allowed.push((Ended(end.clone()), TurnStarted, Running));
        allowed.push((Ended(end.clone()), ConversationMoved, Idle));
        allowed.push((Queued, TurnEnded(end.clone()), Ended(end.clone())));
        allowed.push((Running, TurnEnded(end.clone()), Ended(end.clone())));
        allowed.push((WaitingForInput, TurnEnded(end.clone()), Ended(end)));
    }
    for from in all_states() {
        for event in all_events() {
            let expected = allowed
                .iter()
                .find(|(state, allowed_event, _)| *state == from && *allowed_event == event)
                .map(|(_, _, to)| to.clone())
                .ok_or_else(|| TransitionRefused {
                    from: from.clone(),
                    event: event.clone(),
                });
            assert_eq!(from.after(event.clone()), expected, "{from} + {event}");
        }
    }
}
