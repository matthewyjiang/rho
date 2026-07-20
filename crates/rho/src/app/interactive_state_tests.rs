use pretty_assertions::assert_eq;
use rho_sdk::{
    HostChoice, HostInputRequest, HostQuestion, Retryability, RunEvent, SelectionMode, ToolCallId,
};

use super::{
    active_run_disposition, begin_provider_switch, state_after_event, ActiveRunCommand,
    ActiveRunDisposition, InteractiveState, RunPhase, RunState, TransitionState,
};

fn questionnaire_event() -> RunEvent {
    let question = HostQuestion::new(
        "q1",
        "continue?",
        vec![HostChoice::new("yes", "Yes")],
        SelectionMode::One,
    )
    .unwrap();
    RunEvent::HostInputRequested {
        request: HostInputRequest::questionnaire("confirm", vec![question]).unwrap(),
    }
}

#[test]
fn scripted_events_cover_model_tool_questionnaire_and_steering_states() {
    let state = state_after_event(InteractiveState::Idle, &RunEvent::StepStarted { step: 1 });
    assert_eq!(
        state,
        InteractiveState::Run(RunState::Running(RunPhase::Model))
    );

    let state = state_after_event(
        state,
        &RunEvent::ToolStarted {
            call_id: ToolCallId::from_string("call-1").unwrap(),
            name: "questionnaire".into(),
            metadata: Default::default(),
        },
    );
    assert_eq!(
        state,
        InteractiveState::Run(RunState::Running(RunPhase::Tool))
    );

    let state = state_after_event(state, &questionnaire_event());
    assert_eq!(state, InteractiveState::Run(RunState::WaitingForHostInput));

    let steering = InteractiveState::Run(RunState::Running(RunPhase::Steering));
    assert_eq!(
        state_after_event(
            steering,
            &RunEvent::AssistantTextDelta {
                text: "still streaming".into(),
            },
        ),
        steering
    );
    assert_eq!(
        state_after_event(steering, &RunEvent::StepStarted { step: 2 }),
        InteractiveState::Run(RunState::Running(RunPhase::Model))
    );
}

#[test]
fn cancellation_wins_over_tool_questionnaire_and_compaction_events() {
    let cancelling = InteractiveState::Run(RunState::Cancelling(RunPhase::Tool));
    assert_eq!(
        state_after_event(cancelling, &questionnaire_event()),
        cancelling
    );
    assert_eq!(
        state_after_event(
            cancelling,
            &RunEvent::CompactionStarted {
                trigger: rho_sdk::CompactionTrigger::Automatic,
                message_count: 5,
            },
        ),
        cancelling
    );
    assert_eq!(
        state_after_event(cancelling, &RunEvent::StepStarted { step: 2 }),
        cancelling
    );
    assert_eq!(
        state_after_event(
            cancelling,
            &RunEvent::Cancelled {
                revision: rho_sdk::Revision::INITIAL,
            },
        ),
        InteractiveState::Completed
    );
}

#[test]
fn compaction_provider_switch_and_failure_are_explicit_states() {
    assert_eq!(
        state_after_event(
            InteractiveState::Run(RunState::Running(RunPhase::Model)),
            &RunEvent::CompactionStarted {
                trigger: rho_sdk::CompactionTrigger::Automatic,
                message_count: 8,
            },
        ),
        InteractiveState::Run(RunState::Compacting)
    );
    assert_eq!(
        state_after_event(
            InteractiveState::Run(RunState::Compacting),
            &RunEvent::StepStarted { step: 2 },
        ),
        InteractiveState::Run(RunState::Running(RunPhase::Model))
    );
    assert_eq!(
        begin_provider_switch(InteractiveState::Idle).unwrap(),
        InteractiveState::Transition(TransitionState::SwitchingProvider)
    );
    assert!(
        begin_provider_switch(InteractiveState::Run(RunState::Running(RunPhase::Tool))).is_err()
    );
    assert_eq!(
        state_after_event(
            InteractiveState::Run(RunState::Running(RunPhase::Model)),
            &RunEvent::Failed {
                message: "failed".into(),
                retryability: Retryability::Permanent,
            },
        ),
        InteractiveState::Failed
    );
}

#[test]
fn active_tool_commands_cancel_quit_reject_session_switch_and_defer_provider_replace() {
    assert_eq!(
        active_run_disposition(ActiveRunCommand::Quit),
        ActiveRunDisposition::CancelAndWait
    );
    assert_eq!(
        active_run_disposition(ActiveRunCommand::SwitchSession),
        ActiveRunDisposition::RejectUntilFinished
    );
    assert_eq!(
        active_run_disposition(ActiveRunCommand::ReplaceProvider),
        ActiveRunDisposition::DeferUntilFinished
    );
}
