use std::sync::Arc;

use agent_client_protocol::{
    schema::v1::{
        SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
        SessionConfigOptionValue, SessionConfigSelectOptions, SessionId,
        SetSessionConfigOptionRequest,
    },
    ErrorCode,
};
use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelRequest, ModelResponse},
    provider::{ModelProvider, ProviderFuture, ScriptedProvider, ScriptedTurn},
    SessionOptions, UserInput,
};

use super::{
    apply_thought_level, config_options, parse_thought_level_request, selectable_thought_levels,
    THOUGHT_LEVEL_ID,
};
use crate::{app::session_assembly::BuiltSession, config::Config, tools::sdk_registry::AppToolSet};

fn test_config() -> Config {
    Config {
        provider: "test".into(),
        model: "model".into(),
        ..Config::default()
    }
}

struct HangUntilCancel;

impl ModelProvider for HangUntilCancel {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("test", "test", "model")
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            request.cancellation.cancelled().await;
            Err(rho_sdk::ProviderError::interrupted("cancelled"))
        })
    }
}

fn select_current(options: &[SessionConfigOption]) -> &str {
    match &options[0].kind {
        SessionConfigKind::Select(select) => select.current_value.0.as_ref(),
        SessionConfigKind::Boolean(_) => panic!("thought_level must be a select option"),
        _ => panic!("unexpected session config kind"),
    }
}

fn select_values(options: &[SessionConfigOption]) -> Vec<String> {
    match &options[0].kind {
        SessionConfigKind::Select(select) => match &select.options {
            SessionConfigSelectOptions::Ungrouped(values) => values
                .iter()
                .map(|option| option.value.0.as_ref().to_string())
                .collect(),
            _ => panic!("thought_level must be ungrouped"),
        },
        _ => panic!("thought_level must be a select option"),
    }
}

async fn built_session(
    provider: Arc<dyn ModelProvider>,
    reasoning: ReasoningLevel,
) -> BuiltSession {
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .reasoning_level(reasoning)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    BuiltSession {
        runtime,
        session,
        provider,
        tools: AppToolSet::disabled(),
        hooks: None,
        approval_receiver: None,
    }
}

// Covers: session/new and session/load must advertise thought_level as the
// current reasoning select, not omit it or encode it as a boolean.
// Owner: acp session config mapper
#[test]
fn advertises_thought_level_select_for_the_current_reasoning() {
    let config = test_config();
    let options = config_options(&config, ReasoningLevel::High);
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].id.0.as_ref(), THOUGHT_LEVEL_ID);
    assert_eq!(
        options[0].category,
        Some(SessionConfigOptionCategory::ThoughtLevel)
    );
    assert_eq!(select_current(&options), "high");
    let advertised = select_values(&options);
    let selectable = selectable_thought_levels(&config, ReasoningLevel::High)
        .into_iter()
        .map(|level| level.to_string())
        .collect::<Vec<_>>();
    assert_eq!(advertised, selectable);
    assert!(advertised.contains(&"high".to_string()));
}

// Covers: hosts must get InvalidParams for an unknown option or a non-select
// thought_level value instead of a silent no-op.
// Owner: acp session config mapper
#[test]
fn parse_thought_level_rejects_unknown_option_and_non_select_values() {
    let session = SessionId::new("sess");
    let cases = [
        SetSessionConfigOptionRequest::new(session.clone(), "mode", "bypass"),
        SetSessionConfigOptionRequest::new(
            session.clone(),
            THOUGHT_LEVEL_ID,
            SessionConfigOptionValue::boolean(true),
        ),
        SetSessionConfigOptionRequest::new(session, THOUGHT_LEVEL_ID, "ludicrous"),
    ];
    for request in cases {
        let error = parse_thought_level_request(&request).expect_err("invalid request");
        assert_eq!(error.code, ErrorCode::InvalidParams);
    }
}

// Covers: a host-selected Rho reasoning id must apply to the next idle turn.
// Owner: acp session config mapper
#[tokio::test]
async fn apply_thought_level_updates_the_idle_session() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    );
    let boxed: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    let built = built_session(boxed, rho_sdk::ReasoningLevel::Medium).await;
    let config = test_config();
    let response =
        apply_thought_level(&built, &config, ReasoningLevel::High).expect("apply thought_level");

    built.session.complete("next").await.unwrap();
    assert_eq!(
        built.session.reasoning_level(),
        rho_sdk::ReasoningLevel::High
    );
    assert_eq!(select_current(&response.config_options), "high");
    assert_eq!(
        provider.recorded_requests()[0].reasoning_level,
        rho_sdk::ReasoningLevel::High
    );
}

// Covers: changing thought_level while a prompt is running must fail as busy
// instead of racing the in-flight turn.
// Owner: acp session config mapper
#[tokio::test]
async fn apply_thought_level_rejects_a_busy_session() {
    let provider: Arc<dyn ModelProvider> = Arc::new(HangUntilCancel);
    let built = built_session(provider, rho_sdk::ReasoningLevel::Medium).await;
    let mut run = built.session.start(UserInput::text("hold")).await.unwrap();
    let config = test_config();
    let error =
        apply_thought_level(&built, &config, ReasoningLevel::High).expect_err("busy session");
    run.cancel();
    let _ = run.outcome().await;
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        error.data,
        Some(serde_json::json!(format!(
            "session '{}' already has an active prompt",
            built.session.id()
        )))
    );
}

// Covers: an unsupported pin that is not the current level must reach
// resolve_thought_level instead of the pre-validation "unknown" error.
// Owner: acp session config mapper
#[tokio::test]
async fn apply_thought_level_surfaces_supported_levels_for_unsupported_pins() {
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("poolside", "poolside", "model"),
        [],
    ));
    let built = built_session(provider, rho_sdk::ReasoningLevel::Off).await;
    let config = Config {
        provider: "poolside".into(),
        model: "model".into(),
        ..Config::default()
    };
    let error =
        apply_thought_level(&built, &config, ReasoningLevel::High).expect_err("unsupported pin");
    assert_eq!(error.code, ErrorCode::InvalidParams);
    assert_eq!(
        error.data,
        Some(serde_json::json!(
            "provider 'poolside' model 'model' does not support reasoning level 'high'; supported levels: off, max"
        ))
    );
    assert_eq!(
        built.session.reasoning_level(),
        rho_sdk::ReasoningLevel::Off
    );
}
