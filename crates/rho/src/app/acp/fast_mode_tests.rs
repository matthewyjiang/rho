use agent_client_protocol::{
    schema::v1::{
        SessionConfigOptionCategory, SessionConfigOptionValue, SessionId,
        SetSessionConfigOptionRequest,
    },
    ErrorCode,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{config_options, parse_fast_mode_request, FAST_MODE_ID};
use crate::app::{acp::host_features::Toggles, session_runtime::CurrentModel};

fn current(provider: &str, model: &str, auth: &str) -> CurrentModel {
    CurrentModel {
        provider: provider.into(),
        model: model.into(),
        auth: auth.into(),
    }
}

// Covers: fast_mode must appear only for models that implement fast mode,
// report whether the session is in fast mode as its current value in the
// shape the host accepts (a boolean only for hosts that advertise boolean
// options), and stay out of the `mode` category (T3 hides its
// permission-mode picker when any option uses it).
// Owner: acp session config mapper
#[test]
fn advertises_fast_mode_in_the_host_shape_only_for_fast_capable_models() {
    let codex = current("openai-codex", "gpt-5.5", "codex");
    let cases = [
        (
            &codex,
            false,
            Toggles::Select,
            Some(json!(["select", "off"])),
        ),
        (&codex, true, Toggles::Select, Some(json!(["select", "on"]))),
        (
            &codex,
            false,
            Toggles::Boolean,
            Some(json!(["boolean", false])),
        ),
        (
            &codex,
            true,
            Toggles::Boolean,
            Some(json!(["boolean", true])),
        ),
        (
            &current("xai", "grok-4.7", "xai-oauth"),
            true,
            Toggles::Select,
            Some(json!(["select", "on"])),
        ),
        (
            &current("xai", "grok-4.7", "api-key"),
            true,
            Toggles::Boolean,
            None,
        ),
        (
            &current("openai-codex", "gpt-5.4", "codex"),
            true,
            Toggles::Select,
            None,
        ),
    ];
    for (model, fast, toggles, expected) in cases {
        let options = config_options(model, fast, toggles);
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
