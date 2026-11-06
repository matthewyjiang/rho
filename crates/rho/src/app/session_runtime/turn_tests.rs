use std::{sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, Tool, ToolOutput},
    SystemPrompt, UserInput, Workspace,
};
use serde_json::json;
use tokio::{sync::mpsc, time::timeout};

use crate::{
    app::{
        policy::AppPolicy,
        runtime_builder::{build_runtime, RuntimeBuildOptions},
        session_runtime::{
            test_support::unattended_runtime_on, QuestionSupport, Turn, TurnIo, TurnOutcome,
        },
    },
    compaction::CompactionConfig,
    config::Config,
    permission::PermissionMode,
    tools::sdk_registry::{AppToolSet, StaticToolBundle},
};

/// Bounds each wait on an explicit signal; never the way a test syncs.
const TEST_BUDGET: Duration = Duration::from_secs(5);

// Covers: a run that crosses its compaction threshold mid-turn compacts
// automatically and still finishes with the model's reply.
// Owner: session runtime turn (automatic compaction within a run)
#[tokio::test]
async fn a_run_that_crosses_its_compaction_threshold_compacts_and_finishes() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "turn-compaction"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall::new("call-1".into(), "expand_context".into(), json!({})),
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "compact summary".into(),
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(ScriptedTool::new(
        ToolSpec {
            name: "expand_context".into(),
            description: "return a large result".into(),
            input_schema: json!({"type": "object"}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text("tool context ".repeat(500))),
    ))];
    let mut inventory = AppToolSet::disabled();
    inventory.add_bundle(StaticToolBundle::new(tools));
    let root = tempfile::tempdir().unwrap();
    let config = Config::default();
    let shared: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    let sdk = build_runtime(RuntimeBuildOptions {
        provider: Arc::clone(&shared),
        tools: &inventory,
        workspace: Workspace::new(root.path()).unwrap(),
        workspace_policy: AppPolicy::for_mode(PermissionMode::Auto, Default::default()),
        approval_session: None,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Off,
        service_tier: None,
        compaction: CompactionConfig {
            auto_compact: true,
            threshold_percent: 5,
            target_percent: 1,
            summarizer: None,
        },
        context_window: Some(1_000),
        usage_purpose: "agent",
        usage_parent_session_id: None,
        usage_recording: Default::default(),
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        hooks: None,
        diagnostics: crate::diagnostics::test_diagnostics("test", "test"),
        recall: None,
    })
    .unwrap();
    assert_eq!(sdk.diagnostics().compaction_trigger_tokens(), Some(50));
    let mut runtime = unattended_runtime_on(
        root.path(),
        &config,
        QuestionSupport::Unavailable,
        sdk,
        shared,
    )
    .await;
    let prompt = runtime.begin_prompt(&config).await.unwrap();
    let input = UserInput::text("go");
    let turn = Turn {
        display: input.clone(),
        model: input,
        calls: Vec::new(),
    };
    // Nothing reads the events, and the control sender stays open so the
    // turn has a host.
    let (events, _shown) = mpsc::unbounded_channel();
    let (_controls, control) = mpsc::unbounded_channel();

    let ended = timeout(
        TEST_BUDGET,
        runtime.run_turn(&prompt, turn, TurnIo { events, control }, &config),
    )
    .await
    .expect("the turn ends")
    .unwrap();

    let reply = match ended {
        TurnOutcome::Completed(outcome) => outcome.text().to_owned(),
        TurnOutcome::Cancelled => panic!("the turn was cancelled"),
    };
    let requests = provider.recorded_requests();
    let summary = requests[2]
        .messages
        .iter()
        .find_map(|message| match message {
            Message::CompactionSummary(summary) => Some(summary.trigger),
            _ => None,
        })
        .expect("compacted history carries a typed summary");
    assert_eq!(
        (reply, requests.len(), summary),
        (
            "done".to_owned(),
            3,
            Some(rho_sdk::CompactionTrigger::Automatic)
        )
    );
    drop(prompt);
    runtime.shutdown().await;
}
