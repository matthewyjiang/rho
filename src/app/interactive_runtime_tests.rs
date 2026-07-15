use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    HostChoice, HostInputRequest, HostQuestion, Retryability, RunEvent, SelectionMode,
    SessionOptions, SystemPrompt, ToolCallId, UserInput, Workspace,
};

use super::{
    active_run_disposition, begin_provider_switch, build_runtime, state_after_event,
    ActiveRunCommand, ActiveRunDisposition, InteractiveState, RunPhase,
};
use crate::{compaction::CompactionConfig, tools::sdk_registry::AutomationToolSet};

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
    assert_eq!(state, InteractiveState::Running(RunPhase::Model));

    let state = state_after_event(
        state,
        &RunEvent::ToolStarted {
            call_id: ToolCallId::from_string("call-1").unwrap(),
            name: "questionnaire".into(),
            metadata: Default::default(),
        },
    );
    assert_eq!(state, InteractiveState::Running(RunPhase::Tool));

    let state = state_after_event(state, &questionnaire_event());
    assert_eq!(state, InteractiveState::WaitingForHostInput);

    let steering = InteractiveState::Running(RunPhase::Steering);
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
        InteractiveState::Running(RunPhase::Model)
    );
}

#[test]
fn cancellation_wins_over_tool_questionnaire_and_compaction_events() {
    let cancelling = InteractiveState::Cancelling(RunPhase::Tool);
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
            InteractiveState::Running(RunPhase::Model),
            &RunEvent::CompactionStarted {
                trigger: rho_sdk::CompactionTrigger::Automatic,
                message_count: 8,
            },
        ),
        InteractiveState::Compacting
    );
    assert_eq!(
        state_after_event(
            InteractiveState::Compacting,
            &RunEvent::StepStarted { step: 2 },
        ),
        InteractiveState::Running(RunPhase::Model)
    );
    assert_eq!(
        begin_provider_switch(InteractiveState::Idle).unwrap(),
        InteractiveState::SwitchingProvider
    );
    assert!(begin_provider_switch(InteractiveState::Running(RunPhase::Tool)).is_err());
    assert_eq!(
        state_after_event(
            InteractiveState::Running(RunPhase::Model),
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

#[tokio::test]
async fn configured_token_threshold_installs_sdk_automatic_compaction_policy() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "compact summary".into(),
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let shared_provider: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    let tools = AutomationToolSet::disabled();
    let workspace = Workspace::new(std::env::current_dir().unwrap()).unwrap();
    let runtime = build_runtime(
        shared_provider,
        &tools,
        workspace,
        SystemPrompt::None,
        rho_sdk::ReasoningLevel::Off,
        CompactionConfig {
            auto_compact: true,
            threshold_percent: 1,
            target_percent: 1,
        },
        Some(1_000),
    )
    .unwrap();
    assert_eq!(runtime.diagnostics().compaction_trigger_tokens(), Some(10));
    let session = runtime
        .session(SessionOptions::new().history(vec![
            rho_sdk::model::Message::user_text("x".repeat(2_000)),
            rho_sdk::model::Message::assistant_text("y".repeat(2_000)),
        ]))
        .await
        .unwrap();

    let mut run = session.start(UserInput::text("continue")).await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = run.next_event().await {
        events.push(event);
    }
    let outcome = run.outcome().await.unwrap();

    assert_eq!(outcome.text(), "done");
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::CompactionStarted {
            trigger: rho_sdk::CompactionTrigger::Automatic,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::CompactionCompleted {
            trigger: rho_sdk::CompactionTrigger::Automatic,
            ..
        }
    )));
    assert_eq!(provider.recorded_requests().len(), 2);
}

#[tokio::test]
async fn successful_sdk_completion_reaches_completed_state() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    );
    let runtime = rho_sdk::Rho::builder().provider(provider).build().unwrap();
    let session = runtime.session(Default::default()).await.unwrap();
    let mut run = session.start(rho_sdk::UserInput::text("go")).await.unwrap();
    let mut state = InteractiveState::Idle;
    while let Some(event) = run.next_event().await {
        state = state_after_event(state, &event);
    }
    let outcome = run.outcome().await.unwrap();

    assert_eq!(outcome.text(), "done");
    assert_eq!(state, InteractiveState::Completed);
}
