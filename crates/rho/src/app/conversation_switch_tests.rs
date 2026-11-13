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
        runtime_builder::{build_runtime_with_max_steps, RuntimeBuildOptions},
    },
    compaction::CompactionConfig,
    permission::PermissionMode,
    prompt::{model_switch_context, ModelSwitchKind},
    tools::sdk_registry::AppToolSet,
};

fn identity(provider: &str, model: &str) -> ModelIdentity {
    ModelIdentity::new(provider, "test", model)
}

// Covers: a switch normalizes an unsupported default level, refuses an
// unsupported explicit one, and keeps provenance when the level is unchanged
// (including unknown metadata).
// Owner: conversation model switch policy
#[test]
fn model_switch_reasoning_preserves_explicit_provenance() {
    use super::{resolve_model_switch_reasoning, ModelSwitchReasoningResolution};
    use rho_providers::{
        model::{ReasoningCapabilities, ReasoningLevelSet, ReasoningRequestSource},
        reasoning::ReasoningLevel,
    };

    for source in [
        ReasoningRequestSource::Explicit,
        ReasoningRequestSource::PersistedOrDefault,
    ] {
        for (capabilities, requested, effective) in [
            (
                ReasoningCapabilities::Unknown,
                ReasoningLevel::Off,
                ReasoningLevel::Off,
            ),
            (
                ReasoningCapabilities::NotConfigurable,
                ReasoningLevel::High,
                ReasoningLevel::High,
            ),
            (
                ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
                    ReasoningLevel::Low,
                    ReasoningLevel::High,
                ])),
                ReasoningLevel::High,
                ReasoningLevel::High,
            ),
            (
                ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
                    ReasoningLevel::Low,
                    ReasoningLevel::High,
                ])),
                ReasoningLevel::Medium,
                ReasoningLevel::High,
            ),
            (
                ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
                    ReasoningLevel::Low,
                    ReasoningLevel::High,
                ])),
                ReasoningLevel::Off,
                ReasoningLevel::Low,
            ),
        ] {
            let expected = if requested == effective {
                Ok(ModelSwitchReasoningResolution { effective, source })
            } else if source == ReasoningRequestSource::Explicit {
                Err(requested)
            } else {
                Ok(ModelSwitchReasoningResolution {
                    effective,
                    source: ReasoningRequestSource::PersistedOrDefault,
                })
            };
            assert_eq!(
                resolve_model_switch_reasoning(&capabilities, requested, source),
                expected,
                "{capabilities:?}, {requested:?}, {source:?}",
            );
        }
    }
}

async fn switchable_session(
    history: Vec<Message>,
    context_window: Option<u64>,
) -> (
    rho_sdk::Rho,
    rho_sdk::Session,
    AppToolSet,
    Arc<dyn ModelProvider>,
) {
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        identity("original", "start"),
        Vec::<ScriptedTurn>::new(),
    ));
    let tools = AppToolSet::disabled();
    let workspace = Workspace::new(std::env::current_dir().unwrap()).unwrap();
    let runtime = build_runtime_with_max_steps(
        RuntimeBuildOptions {
            provider: Arc::clone(&provider),
            tools: &tools,
            workspace,
            workspace_policy: AppPolicy::for_mode(
                PermissionMode::Auto,
                Default::default(),
                /*search_path*/ None,
            ),
            approval_session: None,
            system_prompt: SystemPrompt::None,
            reasoning: rho_sdk::ReasoningLevel::Medium,
            service_tier: None,
            compaction: CompactionConfig {
                auto_compact: true,
                threshold_percent: 80,
                target_percent: 50,
                summarizer: None,
            },
            context_window,
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: Default::default(),
            hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
            hooks: None,
            diagnostics: crate::diagnostics::test_diagnostics("test", "test"),
            recall: None,
        },
        /*max_steps*/ None,
    )
    .unwrap();
    let session = runtime
        .session(SessionOptions::new().history(history))
        .await
        .unwrap();
    (runtime, session, tools, provider)
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
    let (_runtime, session, tools, previous_provider) = switchable_session(
        vec![
            Message::user_text("hello"),
            Message::assistant_text("there"),
        ],
        Some(1_000),
    )
    .await;
    let new_provider = replacement("replacement", "next");

    apply_conversation_switch(ConversationSwitch {
        prepared_prompt: None,
        session: &session,
        tools: &tools,
        previous_provider,
        new_provider: Arc::clone(&new_provider),
        new_reasoning: rho_sdk::ReasoningLevel::Low,
        auth: "test-auth",
        compaction: CompactionConfig {
            auto_compact: true,
            threshold_percent: 80,
            target_percent: 50,
            summarizer: None,
        },
        context_window: Some(2_000),
        previous_context_window: Some(1_000),
        usage_recording: Default::default(),
        diagnostics: crate::diagnostics::test_diagnostics("test", "test"),
    })
    .unwrap();

    assert_eq!(
        session.snapshot().provider(),
        &identity("replacement", "next")
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

// Covers: a session with no conversation yet (empty, or only its system
// prompt, as every ACP session has) must not inject a switch notice before the
// first prompt. Owner: conversation switch
#[tokio::test]
async fn fresh_session_switch_stays_silent() {
    for history in [Vec::new(), vec![Message::System("instructions".into())]] {
        let (_runtime, session, tools, previous_provider) =
            switchable_session(history.clone(), None).await;

        apply_conversation_switch(ConversationSwitch {
            prepared_prompt: None,
            session: &session,
            tools: &tools,
            previous_provider,
            new_provider: replacement("replacement", "next"),
            new_reasoning: rho_sdk::ReasoningLevel::Low,
            auth: "test-auth",
            compaction: CompactionConfig::default(),
            context_window: None,
            previous_context_window: None,
            usage_recording: Default::default(),
            diagnostics: crate::diagnostics::test_diagnostics("test", "test"),
        })
        .unwrap();

        assert_eq!(session.history(), history);
        assert_eq!(
            session.snapshot().provider(),
            &identity("replacement", "next")
        );
    }
}
