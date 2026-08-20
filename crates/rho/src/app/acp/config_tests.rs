use std::sync::Arc;

use agent_client_protocol::{
    schema::v1::{
        SessionConfigKind, SessionConfigOptionCategory, SessionId, SetSessionConfigOptionRequest,
    },
    ErrorCode,
};
use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelRequest, ModelResponse},
    provider::{ModelProvider, ProviderFuture, ScriptedProvider, ScriptedTurn},
    ProviderRequestUsageRecording, Rho, SessionOptions, UserInput,
};

use super::{
    apply_thought_level, config_options, parse_thought_level_request, selectable_thought_levels,
    ThoughtLevelApply, THOUGHT_LEVEL_ID,
};
use crate::{compaction::CompactionConfig, config::Config};

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

fn select_current(options: &[agent_client_protocol::schema::v1::SessionConfigOption]) -> &str {
    match &options[0].kind {
        SessionConfigKind::Select(select) => select.current_value.0.as_ref(),
        SessionConfigKind::Boolean(_) => panic!("thought_level must be a select option"),
        _ => panic!("unexpected session config kind"),
    }
}

fn select_values(
    options: &[agent_client_protocol::schema::v1::SessionConfigOption],
) -> Vec<String> {
    match &options[0].kind {
        SessionConfigKind::Select(select) => match &select.options {
            agent_client_protocol::schema::v1::SessionConfigSelectOptions::Ungrouped(values) => {
                values
                    .iter()
                    .map(|option| option.value.0.as_ref().to_string())
                    .collect()
            }
            _ => panic!("thought_level must be ungrouped"),
        },
        _ => panic!("thought_level must be a select option"),
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
            agent_client_protocol::schema::v1::SessionConfigOptionValue::boolean(true),
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
    let runtime = Rho::builder()
        .provider(provider.clone())
        .reasoning_level(rho_sdk::ReasoningLevel::Medium)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let config = test_config();
    let boxed: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    let response = apply_thought_level(
        ThoughtLevelApply {
            session: &session,
            provider: boxed,
            tools: &[],
            compaction: CompactionConfig::default(),
            context_window: None,
            usage_recording: ProviderRequestUsageRecording::default(),
            config: &config,
        },
        ReasoningLevel::High,
    )
    .expect("apply thought_level");

    session.complete("next").await.unwrap();
    assert_eq!(session.reasoning_level(), rho_sdk::ReasoningLevel::High);
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
    let runtime = Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .reasoning_level(rho_sdk::ReasoningLevel::Medium)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("hold")).await.unwrap();
    let config = test_config();
    let error = apply_thought_level(
        ThoughtLevelApply {
            session: &session,
            provider,
            tools: &[],
            compaction: CompactionConfig::default(),
            context_window: None,
            usage_recording: ProviderRequestUsageRecording::default(),
            config: &config,
        },
        ReasoningLevel::High,
    )
    .expect_err("busy session");
    run.cancel();
    let _ = run.outcome().await;
    assert_eq!(error.code, ErrorCode::InvalidRequest);
}
