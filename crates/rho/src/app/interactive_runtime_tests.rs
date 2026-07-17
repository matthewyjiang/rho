use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ModelUsage},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    CompactionFuture, CompactionOutput, CompactionRequest, Compactor, HostChoice, HostInputRequest,
    HostQuestion, ProviderError, ProviderErrorKind, Retryability, RunEvent, RunId, SelectionMode,
    SessionId, SessionOptions, SystemPrompt, ToolCallId, UserInput, Workspace,
};

use super::{
    active_run_disposition, begin_provider_switch, build_runtime, state_after_event,
    ActiveRunCommand, ActiveRunDisposition, InteractiveRuntime, InteractiveState,
    InteractiveWorkspacePolicy, RunPhase, RuntimeBuildOptions,
};
use crate::{
    compaction::CompactionConfig, session::Session as StoredSession,
    tools::sdk_registry::AppToolSet,
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
    let tools = AppToolSet::disabled();
    let workspace = Workspace::new(std::env::current_dir().unwrap()).unwrap();
    let runtime = build_runtime(RuntimeBuildOptions {
        provider: shared_provider,
        tools: tools.tools(),
        workspace,
        workspace_policy: InteractiveWorkspacePolicy,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Off,
        compaction: CompactionConfig {
            auto_compact: true,
            threshold_percent: 1,
            target_percent: 1,
        },
        context_window: Some(1_000),
    })
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

struct PendingCompactor;

impl Compactor for PendingCompactor {
    fn compact<'a>(&'a self, _request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(std::future::pending::<
            Result<CompactionOutput, rho_sdk::Error>,
        >())
    }
}

#[tokio::test]
async fn set_context_window_installs_automatic_compaction_when_idle() {
    let mut interactive = pending_compaction_runtime("done").await;
    interactive.compaction = CompactionConfig {
        auto_compact: true,
        threshold_percent: 1,
        target_percent: 1,
    };
    assert_eq!(
        interactive
            .session
            .diagnostics()
            .compaction_trigger_tokens(),
        None
    );

    interactive.set_context_window(Some(1_000));

    assert_eq!(
        interactive
            .session
            .diagnostics()
            .compaction_trigger_tokens(),
        Some(10)
    );
}

#[tokio::test]
async fn replace_provider_rebuilds_compactor_with_current_context_window() {
    let mut interactive = pending_compaction_runtime("done").await;
    interactive.compaction = CompactionConfig {
        auto_compact: true,
        threshold_percent: 80,
        target_percent: 50,
    };
    interactive.context_window = Some(2_000);
    let replacement: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("replacement", "test", "model"),
        Vec::<ScriptedTurn>::new(),
    ));

    interactive
        .replace_provider(Arc::clone(&replacement), rho_sdk::ReasoningLevel::Low)
        .unwrap();

    assert_eq!(
        interactive
            .session
            .diagnostics()
            .compaction_trigger_tokens(),
        Some(1_600)
    );
    assert_eq!(
        interactive.session.diagnostics().provider(),
        &ModelIdentity::new("replacement", "test", "model")
    );
    assert_eq!(
        interactive.session.reasoning_level(),
        rho_sdk::ReasoningLevel::Low
    );
}

#[tokio::test]
async fn new_sessions_seed_prompt_cache_keys() {
    let provider = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        Vec::<ScriptedTurn>::new(),
    ));
    let tools = AppToolSet::disabled();
    let workspace = Workspace::new(std::env::current_dir().unwrap()).unwrap();
    let runtime = build_runtime(RuntimeBuildOptions {
        provider: Arc::clone(&provider) as Arc<dyn ModelProvider>,
        tools: tools.tools(),
        workspace: workspace.clone(),
        workspace_policy: InteractiveWorkspacePolicy,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Off,
        compaction: CompactionConfig::default(),
        context_window: None,
    })
    .unwrap();
    let id = SessionId::new();
    let cache_key = format!("rho:{}", id.as_str());
    let session = runtime
        .session(
            SessionOptions::new()
                .id(id.clone())
                .prompt_cache_key(cache_key.clone()),
        )
        .await
        .unwrap();

    assert_eq!(
        session.snapshot().prompt_cache_key(),
        Some(cache_key.as_str())
    );
}

async fn test_runtime(turns: Vec<ScriptedTurn>) -> InteractiveRuntime {
    let provider = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        turns,
    ));
    let shared_provider: Arc<dyn ModelProvider> = provider;
    let tools = AppToolSet::disabled();
    let workspace = Workspace::new(std::env::current_dir().unwrap()).unwrap();
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&shared_provider))
        .compactor(PendingCompactor)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    InteractiveRuntime {
        runtime,
        session,
        active_run: None,
        state: InteractiveState::Idle,
        provider: shared_provider,
        tools,
        workspace,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Off,
        compaction: CompactionConfig::default(),
        context_window: None,
        agent_id: "default".into(),
        agent_fingerprint: "test-fingerprint".into(),
        storage: None,
        pending_model_user: None,
        pending_display_user: None,
        pending_history_start: None,
        pending_session_id: None,
        pending_context_usage: None,
        pending_notices: Vec::new(),
        cumulative_input_tokens: 0,
        step_input_token_baseline: 0,
    }
}

async fn pending_compaction_runtime(response: &str) -> InteractiveRuntime {
    test_runtime(vec![ScriptedTurn::completed(ModelResponse::Assistant(
        vec![ContentBlock::Text(response.into())],
    ))])
    .await
}

#[tokio::test]
async fn a_new_run_resets_the_context_usage_baseline() {
    let mut interactive = pending_compaction_runtime("done").await;
    interactive.context_window = Some(10_000);
    interactive.cumulative_input_tokens = 50_000;
    interactive.step_input_token_baseline = 50_000;

    interactive.observe_event(&RunEvent::Started {
        run_id: RunId::new(),
        revision: Default::default(),
    });
    interactive.observe_event(&RunEvent::StepStarted { step: 1 });
    interactive.observe_event(&RunEvent::UsageUpdated {
        usage: ModelUsage {
            input_tokens: Some(300),
            cache_read_tokens: Some(700),
            ..ModelUsage::default()
        },
    });

    assert_eq!(
        interactive.pending_context_usage,
        Some(rho_sdk::model::ContextUsage::provider_reported(
            1_000,
            Some(10_000)
        ))
    );
}

#[tokio::test]
async fn dropping_manual_compaction_does_not_leave_the_runtime_busy() {
    let mut interactive = pending_compaction_runtime("done").await;

    let mut compact = Box::pin(interactive.compact());
    tokio::select! {
        result = &mut compact => panic!("compaction unexpectedly completed: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
    drop(compact);

    interactive
        .start(UserInput::text("continue"), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn failed_turn_does_not_duplicate_the_previous_assistant_in_display_history() {
    let mut interactive = test_runtime(vec![
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            "previous answer".into(),
        )])),
        ScriptedTurn::failed(ProviderError::new(
            ProviderErrorKind::Unavailable,
            "provider unavailable",
            Retryability::Permanent,
        )),
    ])
    .await;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let storage = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    interactive.session = interactive
        .runtime
        .session(SessionOptions::new().id(SessionId::from_string(storage.id()).unwrap()))
        .await
        .unwrap();
    interactive.storage = Some(storage.clone());

    interactive
        .start(UserInput::text("successful prompt"), None)
        .await
        .unwrap();
    while interactive.next_event().await.is_some() {}
    interactive.finish_run().await.unwrap();

    interactive
        .start(UserInput::text("failed prompt"), None)
        .await
        .unwrap();
    while interactive.next_event().await.is_some() {}
    assert!(interactive.finish_run().await.is_err());
    let committed_assistant = interactive.history()[1].clone();

    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(root.path(), &cwd, storage.id()).unwrap();
    assert_eq!(
        histories.display,
        vec![
            Message::user_text("successful prompt"),
            committed_assistant,
            Message::user_text("failed prompt"),
        ]
    );
}

#[tokio::test]
async fn failed_resume_preserves_the_current_runtime() {
    let mut interactive = pending_compaction_runtime("still available").await;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let target = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    std::fs::write(
        target.path(),
        format!(
            "{}\n",
            serde_json::json!({
                "type": "session",
                "version": 999,
                "id": target.id(),
                "timestamp": "1",
                "cwd": cwd,
            })
        ),
    )
    .unwrap();

    assert!(interactive.resume(target, Vec::new()).await.is_err());
    interactive
        .start(UserInput::text("continue"), None)
        .await
        .unwrap();
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
