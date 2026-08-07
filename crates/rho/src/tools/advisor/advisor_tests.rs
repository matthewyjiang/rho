use std::num::NonZeroUsize;

use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    tool::{tool_progress_channel, Tool, ToolErrorKind},
    CancellationToken, SessionId,
};
use serde_json::json;

use crate::config::{Config, InternalAgentModelConfig};

use super::{
    advisor_effective_reasoning, advisor_model, advisor_progress_message,
    consult_advisor_with_provider, AdvisorSessionStore, AdvisorTool, DEFAULT_TRANSCRIPT_BUDGET,
    NO_MODEL_MESSAGE, NO_SESSION_MESSAGE, TOOL_NAME,
};

fn advisor_selection() -> InternalAgentModelConfig {
    InternalAgentModelConfig::new("anthropic".into(), "claude-test".into(), "api-key".into())
}

fn config_with(advisor_mode: bool, model: Option<InternalAgentModelConfig>) -> Config {
    let mut config = Config {
        advisor_mode,
        ..Config::default()
    };
    if let Some(model) = model {
        config.set_internal_agent_model(
            crate::agent::ADVISOR_AGENT_ID,
            model.provider,
            model.model,
            model.auth,
        );
    }
    config
}

// Covers: the advisor is the one internal agent with no conversation-model
// fallback, so an unset advisor model must stay unset.
// Owner: advisor tool configuration
#[test]
fn the_advisor_model_never_falls_back_to_the_conversation_model() {
    let config = config_with(true, None);

    assert_eq!(advisor_model(&config), None);
    assert_eq!(
        advisor_model(&config_with(true, Some(advisor_selection()))).map(|model| &model.model),
        Some(&"claude-test".to_string())
    );
}

// Covers: unset advisor reasoning keeps the reserved definition default.
// Owner: advisor tool configuration
#[test]
fn advisor_reasoning_defaults_to_the_definition_level() {
    assert_eq!(
        advisor_effective_reasoning(&advisor_selection()),
        ReasoningLevel::Medium
    );
}

// Covers: an explicit advisor reasoning override wins over the definition default.
// Owner: advisor tool configuration
#[test]
fn advisor_reasoning_override_wins() {
    let mut selection = advisor_selection();
    selection.reasoning = Some(ReasoningLevel::High);
    assert_eq!(
        advisor_effective_reasoning(&selection),
        ReasoningLevel::High
    );
}

#[test]
fn a_request_without_a_model_reports_how_to_choose_one() {
    let store = AdvisorSessionStore::new();

    let error = store
        .request(DEFAULT_TRANSCRIPT_BUDGET)
        .expect_err("a store with no model cannot build a request");

    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert_eq!(error.message(), NO_MODEL_MESSAGE);
}

#[test]
fn a_request_without_a_session_reports_the_missing_session() {
    let store = AdvisorSessionStore::new();
    store.set_model(Some(advisor_selection()));

    let error = store
        .request(DEFAULT_TRANSCRIPT_BUDGET)
        .expect_err("a store with no session cannot build a request");

    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert_eq!(error.message(), NO_SESSION_MESSAGE);
}

#[test]
fn the_tool_takes_no_arguments() {
    let tool = AdvisorTool::new(AdvisorSessionStore::new(), DEFAULT_TRANSCRIPT_BUDGET);

    let spec = tool.spec();

    assert_eq!(spec.name, TOOL_NAME);
    assert_eq!(
        spec.input_schema,
        json!({ "type": "object", "additionalProperties": false, "properties": {} })
    );
}

// Covers: finished advisor spend must accumulate and claim once for the parent
// session total (statusline and /info).
// Owner: advisor cost ledger
#[test]
fn advisor_costs_accumulate_and_claim_once() {
    use rho_sdk::model::ModelUsage;

    let store = AdvisorSessionStore::new();
    store.note_usage(&ModelUsage {
        cost_usd_micros: Some(12_500),
        ..ModelUsage::default()
    });
    store.note_usage(&ModelUsage {
        cost_usd_micros: Some(7_500),
        ..ModelUsage::default()
    });
    // Tokens without a provider cost stay silent.
    store.note_usage(&ModelUsage {
        input_tokens: Some(100),
        ..ModelUsage::default()
    });

    assert_eq!(store.claim_cost_usd_micros(), 20_000);
    assert_eq!(store.claim_cost_usd_micros(), 0);
}

// Covers: session rebinds keep or drop unclaimed spend by session id.
// Owner: advisor cost ledger
#[tokio::test]
async fn rebinding_session_scopes_unclaimed_advisor_cost() {
    use rho_sdk::{
        model::{ContentBlock, ModelIdentity, ModelResponse, ModelUsage},
        provider::{ScriptedProvider, ScriptedTurn},
        Rho, SessionOptions, Workspace,
    };

    let root = tempfile::tempdir().unwrap();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("unused".into()),
        ]))],
    );
    let rho = Rho::builder()
        .provider(provider)
        .workspace(Workspace::new(root.path()).unwrap())
        .build()
        .unwrap();
    let first = rho.session(SessionOptions::default()).await.unwrap();
    let second = rho.session(SessionOptions::default()).await.unwrap();
    assert_ne!(first.id(), second.id());

    let store = AdvisorSessionStore::new();
    store.bind_session(first.clone());
    store.note_usage(&ModelUsage {
        cost_usd_micros: Some(9_000),
        ..ModelUsage::default()
    });
    assert_eq!(store.unclaimed_cost_usd_micros(), 9_000);

    // Same-id rebind (policy rebuild) keeps the accumulator.
    store.bind_session(first);
    assert_eq!(store.unclaimed_cost_usd_micros(), 9_000);

    // A new conversation must not inherit the previous total.
    store.bind_session(second);
    assert_eq!(store.unclaimed_cost_usd_micros(), 0);
}

// Covers: partial advisor progress is visible while the provider is still
// running, reasoning stays hidden, and the final tool output is plain guidance
// Owner: advisor tool progress bridge
#[tokio::test]
async fn streams_progress_before_completion_with_plain_final_output() {
    use std::sync::Mutex;

    use rho_sdk::{
        model::{ModelEvent, ModelRequest},
        provider::{ModelProvider, ProviderEventSender, ProviderFuture},
    };

    struct GatedAdvisorProvider {
        identity: ModelIdentity,
        partial_ready: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl ModelProvider for GatedAdvisorProvider {
        fn identity(&self) -> ModelIdentity {
            self.identity.clone()
        }

        fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
            Box::pin(async {
                Err(rho_sdk::ProviderError::interrupted(
                    "gated advisor provider is stream-only",
                ))
            })
        }

        fn send_turn_stream<'a>(
            &'a self,
            request: ModelRequest<'a>,
            events: ProviderEventSender,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                if request.cancellation.is_cancelled() {
                    return Err(rho_sdk::ProviderError::interrupted(
                        "provider request cancelled",
                    ));
                }
                events
                    .send(ModelEvent::ReasoningDelta("do not show".into()))
                    .await?;
                events
                    .send(ModelEvent::OutputDelta("prefer ".into()))
                    .await?;
                if let Some(ready) = self.partial_ready.lock().unwrap().take() {
                    let _ = ready.send(());
                }
                let release = self
                    .release
                    .lock()
                    .unwrap()
                    .take()
                    .expect("release gate installed");
                tokio::select! {
                    result = release => {
                        result.expect("release gate closed");
                    }
                    () = request.cancellation.cancelled() => {
                        return Err(rho_sdk::ProviderError::interrupted(
                            "provider request cancelled",
                        ));
                    }
                }
                events
                    .send(ModelEvent::OutputDelta("the simple path".into()))
                    .await?;
                Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "prefer the simple path".into(),
                )]))
            })
        }
    }

    let (partial_ready_tx, partial_ready_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let provider = GatedAdvisorProvider {
        identity: ModelIdentity::new("provider", "api", "model"),
        partial_ready: Mutex::new(Some(partial_ready_tx)),
        release: Mutex::new(Some(release_rx)),
    };
    let (progress_tx, mut progress_rx) = tool_progress_channel(NonZeroUsize::new(16).unwrap());
    let session_id = SessionId::new();

    let consult = consult_advisor_with_provider(
        &provider,
        &session_id,
        std::path::Path::new("/test/workspace"),
        "transcript".into(),
        ReasoningLevel::Medium,
        CancellationToken::new(),
        progress_tx,
    );
    tokio::pin!(consult);

    // Drive the consultation while waiting for the provider's partial gate.
    tokio::select! {
        ready = partial_ready_rx => {
            ready.expect("partial progress gate");
        }
        result = &mut consult => {
            panic!("advisor finished before partial progress gate: {result:?}");
        }
    }
    let mut saw_partial = false;
    while !saw_partial {
        tokio::select! {
            progress = progress_rx.recv() => {
                let progress = progress.expect("progress channel closed before partial output");
                assert!(
                    !progress.text().contains("do not show"),
                    "reasoning leaked into progress: {}",
                    progress.text()
                );
                if progress.presentation().command_summary_text() == Some("responding")
                    && progress.text().contains("prefer")
                {
                    saw_partial = true;
                    // Provider is still gated; completion text must not be present yet.
                    assert!(!progress.text().contains("simple path"));
                }
            }
            result = &mut consult => {
                panic!("advisor finished before partial progress was observed: {result:?}");
            }
        }
    }

    release_tx.send(()).expect("release gated provider");
    let (advice, _usage) = consult.await.expect("advisor consultation");

    assert_eq!(advice, "prefer the simple path");
    assert!(!advice.contains("responding"));
    assert!(!advice.contains("thinking"));
}

// Covers: progress messages keep phase in metadata and body as plain text
// Owner: advisor tool progress bridge
#[test]
fn progress_message_keeps_phase_out_of_body() {
    use crate::agent::{OneShotPhase, OneShotUpdate};

    let progress = advisor_progress_message(&OneShotUpdate::new(
        OneShotPhase::Responding,
        "ship the smaller change",
    ));
    assert_eq!(progress.text(), "ship the smaller change");
    assert_eq!(
        progress.presentation().command_summary_text(),
        Some("responding")
    );
}
