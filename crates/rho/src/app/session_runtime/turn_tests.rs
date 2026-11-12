use std::{sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{
        AssistantMessage, ContentBlock, Message, MessageTime, ModelEvent, ModelIdentity,
        ModelResponse, ProviderContextBlock, ReplayScope, ToolCall, ToolSpec,
    },
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, Tool, ToolOutput},
    CancellationToken, Rho, SessionOptions, SessionRequestPrefix, SystemPrompt, UserInput,
    Workspace,
};
use serde_json::json;
use tokio::{sync::mpsc, time::timeout};

use crate::{
    app::{
        policy::AppPolicy,
        runtime_builder::{build_runtime, RuntimeBuildOptions},
        session_runtime::{
            test_support::unattended_runtime_on, QuestionSupport, SessionRuntime, Turn, TurnIo,
            TurnOutcome,
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
    let sdk = agent_runtime(
        Arc::clone(&shared),
        &inventory,
        root.path(),
        CompactionConfig {
            auto_compact: true,
            threshold_percent: 5,
            target_percent: 1,
            summarizer: None,
        },
    );
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

/// The SDK runtime the host builds for a root agent on `provider` with
/// `tools`, working in `root`.
fn agent_runtime(
    provider: Arc<dyn ModelProvider>,
    tools: &AppToolSet,
    root: &std::path::Path,
    compaction: CompactionConfig,
) -> Rho {
    build_runtime(RuntimeBuildOptions {
        provider,
        tools,
        workspace: Workspace::new(root).unwrap(),
        workspace_policy: AppPolicy::for_mode(
            PermissionMode::Auto,
            Default::default(),
            /*search_path*/ None,
        ),
        approval_session: None,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Off,
        service_tier: None,
        compaction,
        context_window: Some(1_000),
        usage_purpose: "agent",
        usage_parent_session_id: None,
        usage_recording: Default::default(),
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        hooks: None,
        diagnostics: crate::diagnostics::test_diagnostics("test", "test"),
        recall: None,
    })
    .unwrap()
}

/// Compaction that never runs, so requests carry history as it was saved.
fn no_compaction() -> CompactionConfig {
    CompactionConfig {
        auto_compact: false,
        threshold_percent: 80,
        target_percent: 40,
        summarizer: None,
    }
}

/// Runs one prompt of `text` to completion on `runtime`.
async fn run_prompt(runtime: &mut SessionRuntime, config: &Config, text: &str) {
    let prompt = runtime.begin_prompt(config).await.unwrap();
    let input = UserInput::text(text);
    let turn = Turn {
        display: input.clone(),
        model: input,
        calls: Vec::new(),
    };
    let (events, _shown) = mpsc::unbounded_channel();
    let (_controls, control) = mpsc::unbounded_channel();
    let ended = timeout(
        TEST_BUDGET,
        runtime.run_turn(&prompt, turn, TurnIo { events, control }, config),
    )
    .await
    .expect("the turn ends")
    .unwrap();
    assert!(matches!(ended, TurnOutcome::Completed(_)), "{ended:?}");
}

/// The exact bytes `prefix` would send as messages.
fn request_bytes(prefix: &SessionRequestPrefix) -> String {
    serde_json::to_string(prefix.as_request(CancellationToken::new()).messages).unwrap()
}

// Covers: a provider request shows when each user message and assistant text
// output was created, on its own line, and leaves thinking as the provider
// sent it; a session resumed from its file sends the same bytes as before,
// so the prompt cache survives a reload.
// Owner: session runtime turn (message timestamps)
#[tokio::test]
async fn requests_carry_saved_message_times_across_resume() {
    let identity = ModelIdentity::new("test", "test", "timestamps");
    let thinking = json!({"type": "thinking", "thinking": "weighing it", "signature": "sig"});
    let provider = ScriptedProvider::new(
        identity.clone(),
        [
            ScriptedTurn::streaming(
                vec![ModelEvent::ProviderContext {
                    kind: "anthropic_content_block".into(),
                    position: Some(0),
                    data: thinking.clone(),
                    replay: ReplayScope::ExactModel,
                }],
                ModelResponse::Assistant(vec![ContentBlock::Text("first answer".into())]),
            ),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "second answer".into(),
            )])),
        ],
    );
    let root = tempfile::tempdir().unwrap();
    let config = Config::default();
    let shared: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    let tools = AppToolSet::disabled();
    let sdk = agent_runtime(Arc::clone(&shared), &tools, root.path(), no_compaction());
    let mut runtime = unattended_runtime_on(
        root.path(),
        &config,
        QuestionSupport::Unavailable,
        sdk,
        Arc::clone(&shared),
    )
    .await;

    // Every stamp is one of these, a minute apart at most.
    let before = MessageTime::now();
    run_prompt(&mut runtime, &config, "hello").await;
    run_prompt(&mut runtime, &config, "again").await;
    let window = [before, MessageTime::now()];

    let requests = provider.recorded_requests();
    let sent = &requests[1].messages;
    let user = |text: &str| {
        window
            .iter()
            .map(|time| Message::user_text(format!("{time}\n{text}")))
            .find(|message| sent.contains(message))
            .unwrap_or_else(|| panic!("no stamped {text:?} in {sent:#?}"))
    };
    let answered = window
        .iter()
        .find(|time| sent[1] == stamped_answer(time, &identity, &thinking))
        .unwrap_or_else(|| panic!("no stamped answer in {sent:#?}"));
    assert_eq!(
        sent,
        &vec![
            user("hello"),
            stamped_answer(answered, &identity, &thinking),
            user("again"),
        ]
    );

    let live = runtime.built.session.request_prefix().unwrap();
    let stored = runtime.storage.file().expect("the session is saved");
    let snapshot = stored
        .snapshot_for_resume(identity, "resume".into())
        .unwrap();
    let resumed = agent_runtime(shared, &tools, root.path(), no_compaction())
        .session(SessionOptions::from_snapshot(snapshot))
        .await
        .unwrap();
    assert_eq!(
        request_bytes(&resumed.request_prefix().unwrap()),
        request_bytes(&live)
    );
    runtime.shutdown().await;
}

/// The first answer as a request sends it: stamped with `time`, its thinking
/// untouched.
fn stamped_answer(
    time: &MessageTime,
    identity: &ModelIdentity,
    thinking: &serde_json::Value,
) -> Message {
    Message::assistant(AssistantMessage {
        content: vec![ContentBlock::Text(format!("{time}\nfirst answer"))],
        provenance: Some(identity.clone()),
        reasoning_summary: None,
        provider_context: vec![ProviderContextBlock {
            identity: identity.clone(),
            kind: "anthropic_content_block".into(),
            position: Some(0),
            data: thinking.clone(),
            replay: ReplayScope::ExactModel,
        }],
        created_at: None,
    })
}
