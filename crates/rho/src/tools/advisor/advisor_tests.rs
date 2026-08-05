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
