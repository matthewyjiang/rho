use std::sync::Arc;

use agent_client_protocol::{
    schema::v1::{
        SessionConfigKind, SessionConfigOptionCategory, SessionConfigOptionValue, SessionId,
        SetSessionConfigOptionRequest,
    },
    ErrorCode,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ServiceTier},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    SessionOptions,
};

use super::{apply_fast_mode, config_options, parse_fast_mode_request, FAST_MODE_ID};
use crate::app::acp::config_options::CurrentModel;

fn current(provider: &str, model: &str, auth: &str) -> CurrentModel {
    CurrentModel {
        provider: provider.into(),
        model: model.into(),
        auth: auth.into(),
    }
}

// Covers: fast_mode must appear only for models that implement fast mode,
// report the session's tier as its current value, and stay out of the `mode`
// category (T3 hides its permission-mode picker when any option uses it).
// Owner: acp session config mapper
#[test]
fn advertises_fast_mode_only_for_fast_capable_models() {
    let cases = [
        (
            current("openai-codex", "gpt-5.5", "codex"),
            None,
            Some("off"),
        ),
        (
            current("openai-codex", "gpt-5.5", "codex"),
            Some(ServiceTier::Priority),
            Some("on"),
        ),
        (
            current("xai", "grok-4.7", "xai-oauth"),
            Some(ServiceTier::Priority),
            Some("on"),
        ),
        (
            current("xai", "grok-4.7", "api-key"),
            Some(ServiceTier::Priority),
            None,
        ),
        (
            current("openai-codex", "gpt-5.4", "codex"),
            Some(ServiceTier::Priority),
            None,
        ),
    ];
    for (model, tier, expected) in cases {
        let options = config_options(&model, tier);
        let advertised = options.first().map(|option| {
            assert_eq!(option.id.0.as_ref(), FAST_MODE_ID);
            assert_eq!(
                option.category,
                Some(SessionConfigOptionCategory::ModelConfig)
            );
            match &option.kind {
                SessionConfigKind::Select(select) => select.current_value.0.to_string(),
                other => panic!("fast_mode must be a select, got {other:?}"),
            }
        });
        assert_eq!(
            advertised.as_deref(),
            expected,
            "{}/{} via {}",
            model.provider,
            model.model,
            model.auth
        );
    }
}

// Covers: hosts must get InvalidParams for a value other than on/off instead
// of a silent no-op.
// Owner: acp session config mapper
#[test]
fn parse_fast_mode_accepts_only_on_and_off() {
    let session = SessionId::new("sess");
    let cases = [
        (SessionConfigOptionValue::from("on"), Ok(true)),
        (SessionConfigOptionValue::from("off"), Ok(false)),
        (
            SessionConfigOptionValue::from("ludicrous"),
            Err(ErrorCode::InvalidParams),
        ),
        (
            SessionConfigOptionValue::boolean(true),
            Err(ErrorCode::InvalidParams),
        ),
    ];
    for (value, expected) in cases {
        let request = SetSessionConfigOptionRequest::new(session.clone(), FAST_MODE_ID, value);
        let parsed = parse_fast_mode_request(&request).map_err(|error| error.code);
        assert_eq!(parsed, expected, "{:?}", request.value);
    }
}

// Covers: a host toggle must reach the next model request's service tier;
// enabling it on a model without fast mode is rejected (even when a tier
// carried across a model switch already matches), while disabling that
// carried tier always succeeds.
// Owner: acp session config mapper
#[tokio::test]
async fn apply_fast_mode_sets_the_next_request_service_tier() {
    let codex = current("openai-codex", "gpt-5.5", "codex");
    let plain = current("test", "model", "api-key");
    let priority = Some(ServiceTier::Priority);
    let cases = [
        (&codex, None, true, Ok(priority)),
        (&codex, priority, false, Ok(None)),
        (&plain, None, true, Err(ErrorCode::InvalidParams)),
        (&plain, priority, true, Err(ErrorCode::InvalidParams)),
        (&plain, priority, false, Ok(None)),
    ];
    for (model, start, enabled, expected) in cases {
        let provider = ScriptedProvider::new(
            ModelIdentity::new("test", "test", "model"),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("done".into()),
            ]))],
        );
        let shared: Arc<dyn ModelProvider> = Arc::new(provider.clone());
        let mut builder = rho_sdk::Rho::builder().provider_shared(shared);
        if let Some(tier) = start {
            builder = builder.service_tier(tier);
        }
        let runtime = builder.build().unwrap();
        let session = runtime.session(SessionOptions::default()).await.unwrap();

        let result = apply_fast_mode(&session, model, enabled).map_err(|error| error.code);
        let label = format!("{}/{} enabled={enabled}", model.provider, model.model);
        match expected {
            Ok(tier) => {
                assert_eq!(result, Ok(()), "{label}");
                session.complete("next").await.unwrap();
                assert_eq!(
                    provider.recorded_requests()[0].service_tier,
                    tier,
                    "{label}"
                );
            }
            Err(code) => {
                assert_eq!(result, Err(code), "{label}");
                assert_eq!(session.service_tier(), start, "{label}");
            }
        }
    }
}
