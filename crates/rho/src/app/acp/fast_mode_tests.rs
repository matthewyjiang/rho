use std::sync::Arc;

use agent_client_protocol::{
    schema::v1::{
        SessionConfigOptionCategory, SessionConfigOptionValue, SessionId,
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
use serde_json::json;

use super::{apply_fast_mode, config_options, parse_fast_mode_request, FAST_MODE_ID};
use crate::app::acp::{config_options::CurrentModel, host_features::Toggles};

fn current(provider: &str, model: &str, auth: &str) -> CurrentModel {
    CurrentModel {
        provider: provider.into(),
        model: model.into(),
        auth: auth.into(),
    }
}

// Covers: fast_mode must appear only for models that implement fast mode,
// report the session's tier as its current value in the shape the host
// accepts (a boolean only for hosts that advertise boolean options), and stay
// out of the `mode` category (T3 hides its permission-mode picker when any
// option uses it).
// Owner: acp session config mapper
#[test]
fn advertises_fast_mode_in_the_host_shape_only_for_fast_capable_models() {
    let codex = current("openai-codex", "gpt-5.5", "codex");
    let priority = Some(ServiceTier::Priority);
    let cases = [
        (
            &codex,
            None,
            Toggles::Select,
            Some(json!(["select", "off"])),
        ),
        (
            &codex,
            priority,
            Toggles::Select,
            Some(json!(["select", "on"])),
        ),
        (
            &codex,
            None,
            Toggles::Boolean,
            Some(json!(["boolean", false])),
        ),
        (
            &codex,
            priority,
            Toggles::Boolean,
            Some(json!(["boolean", true])),
        ),
        (
            &current("xai", "grok-4.7", "xai-oauth"),
            priority,
            Toggles::Select,
            Some(json!(["select", "on"])),
        ),
        (
            &current("xai", "grok-4.7", "api-key"),
            priority,
            Toggles::Boolean,
            None,
        ),
        (
            &current("openai-codex", "gpt-5.4", "codex"),
            priority,
            Toggles::Select,
            None,
        ),
    ];
    for (model, tier, toggles, expected) in cases {
        let options = config_options(model, tier, toggles);
        let advertised = options.first().map(|option| {
            assert_eq!(option.id.0.as_ref(), FAST_MODE_ID);
            assert_eq!(
                option.category,
                Some(SessionConfigOptionCategory::ModelConfig)
            );
            let wire = serde_json::to_value(option).unwrap();
            json!([wire["type"], wire["currentValue"]])
        });
        assert_eq!(
            advertised, expected,
            "{}/{} via {} as {toggles:?}",
            model.provider, model.model, model.auth
        );
    }
}

// Covers: hosts must get InvalidParams for a value other than on/off, or for
// a value whose shape differs from the option they were offered, instead of
// a silent no-op.
// Owner: acp session config mapper
#[test]
fn parse_fast_mode_accepts_only_the_offered_shape() {
    let session = SessionId::new("sess");
    let cases = [
        (
            Toggles::Select,
            SessionConfigOptionValue::from("on"),
            Ok(true),
        ),
        (
            Toggles::Select,
            SessionConfigOptionValue::from("off"),
            Ok(false),
        ),
        (
            Toggles::Select,
            SessionConfigOptionValue::from("ludicrous"),
            Err(ErrorCode::InvalidParams),
        ),
        (
            Toggles::Select,
            SessionConfigOptionValue::boolean(true),
            Err(ErrorCode::InvalidParams),
        ),
        (
            Toggles::Boolean,
            SessionConfigOptionValue::boolean(true),
            Ok(true),
        ),
        (
            Toggles::Boolean,
            SessionConfigOptionValue::boolean(false),
            Ok(false),
        ),
        (
            Toggles::Boolean,
            SessionConfigOptionValue::from("on"),
            Err(ErrorCode::InvalidParams),
        ),
    ];
    for (toggles, value, expected) in cases {
        let request = SetSessionConfigOptionRequest::new(session.clone(), FAST_MODE_ID, value);
        let parsed = parse_fast_mode_request(&request, toggles).map_err(|error| error.code);
        assert_eq!(parsed, expected, "{toggles:?} {:?}", request.value);
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
