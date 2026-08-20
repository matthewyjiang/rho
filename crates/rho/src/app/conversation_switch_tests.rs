use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{Message, ModelIdentity},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    SessionOptions, SystemPrompt, Workspace,
};

use super::{apply_conversation_switch, ConversationSwitch};
use crate::{
    app::{
        policy::AppPolicy,
        runtime_builder::{build_runtime, RuntimeBuildOptions},
    },
    compaction::CompactionConfig,
    permission::PermissionMode,
    prompt::{model_switch_context, ModelSwitchKind},
    tools::sdk_registry::AppToolSet,
};

fn identity(provider: &str, model: &str) -> ModelIdentity {
    ModelIdentity::new(provider, "test", model)
}

async fn switchable_session(
    history: Vec<Message>,
    context_window: Option<u64>,
) -> (rho_sdk::Rho, rho_sdk::Session, AppToolSet) {
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        identity("original", "start"),
        Vec::<ScriptedTurn>::new(),
    ));
    let tools = AppToolSet::disabled();
    let workspace = Workspace::new(std::env::current_dir().unwrap()).unwrap();
    let runtime = build_runtime(RuntimeBuildOptions {
        provider,
        tools: tools.tools(),
        workspace,
        workspace_policy: AppPolicy::for_mode(PermissionMode::Auto, Default::default()),
        approval_session: None,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Medium,
        service_tier: None,
        compaction: CompactionConfig {
            auto_compact: true,
            threshold_percent: 80,
            target_percent: 50,
        },
        context_window,
        usage_purpose: "agent",
        usage_parent_session_id: None,
        usage_recording: Default::default(),
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        hooks: None,
    })
    .unwrap();
    let session = runtime
        .session(SessionOptions::new().history(history))
        .await
        .unwrap();
    (runtime, session, tools)
}

fn replacement(provider: &str, model: &str) -> Arc<dyn ModelProvider> {
    Arc::new(ScriptedProvider::new(
        identity(provider, model),
        Vec::<ScriptedTurn>::new(),
    ))
}

// Covers: a mid-session switch must move provider, reasoning, and compaction together
// Owner: conversation switch
#[tokio::test]
async fn mid_session_switch_updates_provider_reasoning_and_compaction() {
    let (_runtime, session, tools) = switchable_session(
        vec![
            Message::user_text("hello"),
            Message::assistant_text("there"),
        ],
        Some(1_000),
    )
    .await;
    let new_provider = replacement("replacement", "next");

    apply_conversation_switch(ConversationSwitch {
        session: &session,
        tools: &tools,
        new_provider: Arc::clone(&new_provider),
        new_reasoning: rho_sdk::ReasoningLevel::Low,
        auth: "test-auth",
        compaction: CompactionConfig {
            auto_compact: true,
            threshold_percent: 80,
            target_percent: 50,
        },
        context_window: Some(2_000),
        previous_context_window: Some(1_000),
        usage_recording: Default::default(),
    })
    .unwrap();

    assert_eq!(
        session.provider().identity(),
        identity("replacement", "next")
    );
    assert_eq!(session.reasoning_level(), rho_sdk::ReasoningLevel::Low);
    assert_eq!(
        session.diagnostics().compaction_trigger_tokens(),
        Some(1_600)
    );
    let (expected_notice, _) = model_switch_context(
        ModelSwitchKind::Conversation,
        &crate::model_identity::PromptModel::from_sdk_identity(&identity("replacement", "next")),
    );
    assert_eq!(
        session.history().last(),
        Some(&Message::user_text(expected_notice))
    );
}

// Covers: an empty session must not inject a switch notice before the first prompt
// Owner: conversation switch
#[tokio::test]
async fn empty_session_switch_stays_silent() {
    let (_runtime, session, tools) = switchable_session(Vec::new(), None).await;

    apply_conversation_switch(ConversationSwitch {
        session: &session,
        tools: &tools,
        new_provider: replacement("replacement", "next"),
        new_reasoning: rho_sdk::ReasoningLevel::Low,
        auth: "test-auth",
        compaction: CompactionConfig::default(),
        context_window: None,
        previous_context_window: None,
        usage_recording: Default::default(),
    })
    .unwrap();

    assert_eq!(session.history(), Vec::<Message>::new());
    assert_eq!(
        session.provider().identity(),
        identity("replacement", "next")
    );
}
