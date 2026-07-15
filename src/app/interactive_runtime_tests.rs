use rho_sdk::{HostChoice, HostInputRequest, HostQuestion, Retryability, RunEvent, SelectionMode};

use super::{state_after_event, InteractiveState};

#[test]
fn state_transitions_cover_running_waiting_cancelling_completion_and_failure() {
    let started = RunEvent::StepStarted { step: 1 };
    assert_eq!(
        state_after_event(InteractiveState::Idle, &started),
        InteractiveState::Running
    );

    let question = HostQuestion::new(
        "q1",
        "continue?",
        vec![HostChoice::new("yes", "Yes")],
        SelectionMode::One,
    )
    .unwrap();
    let waiting = RunEvent::HostInputRequested {
        request: HostInputRequest::questionnaire("confirm", vec![question]).unwrap(),
    };
    assert_eq!(
        state_after_event(InteractiveState::Running, &waiting),
        InteractiveState::WaitingForHostInput
    );
    assert_eq!(
        state_after_event(InteractiveState::Cancelling, &started),
        InteractiveState::Cancelling
    );
    assert_eq!(
        state_after_event(
            InteractiveState::Cancelling,
            &RunEvent::Cancelled {
                revision: rho_sdk::Revision::INITIAL,
            },
        ),
        InteractiveState::Completed
    );
    assert_eq!(
        state_after_event(
            InteractiveState::Running,
            &RunEvent::Failed {
                message: "failed".into(),
                retryability: Retryability::Permanent,
            },
        ),
        InteractiveState::Failed
    );
}
