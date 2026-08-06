use pretty_assertions::assert_eq;
use rho_sdk::{tool::Tool as _, tool::ToolErrorKind};
use serde_json::json;

use crate::config::{Config, InternalAgentModelConfig};

use super::{
    advisor_model, AdvisorSessionStore, AdvisorTool, DEFAULT_TRANSCRIPT_BUDGET, NO_MODEL_MESSAGE,
    NO_SESSION_MESSAGE, TOOL_NAME,
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
