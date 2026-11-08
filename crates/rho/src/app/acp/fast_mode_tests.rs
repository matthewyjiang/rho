use agent_client_protocol::{
    schema::v1::{
        SessionConfigOptionCategory, SessionConfigOptionValue, SessionId,
        SetSessionConfigOptionRequest,
    },
    ErrorCode,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use rho_protocol as wire;

use super::{config_options, parse_fast_mode_request, FAST_MODE_ID};
use crate::app::acp::host_features::Toggles;

// Covers: fast_mode must appear only when the model has fast mode, report
// whether the session is in fast mode as its current value in the shape the
// host accepts (a boolean only for hosts that advertise boolean options),
// and stay out of the `mode` category (T3 hides its permission-mode picker
// when any option uses it).
// Owner: acp session config mapper
#[test]
fn advertises_fast_mode_in_the_host_shape_only_when_available() {
    let off = Some(wire::FastModeOptions { enabled: false });
    let on = Some(wire::FastModeOptions { enabled: true });
    let cases = [
        (&off, Toggles::Select, Some(json!(["select", "off"]))),
        (&on, Toggles::Select, Some(json!(["select", "on"]))),
        (&off, Toggles::Boolean, Some(json!(["boolean", false]))),
        (&on, Toggles::Boolean, Some(json!(["boolean", true]))),
        (&None, Toggles::Select, None),
        (&None, Toggles::Boolean, None),
    ];
    for (fast_mode, toggles, expected) in cases {
        let options = config_options(fast_mode.as_ref(), toggles);
        let advertised = options.first().map(|option| {
            assert_eq!(option.id.0.as_ref(), FAST_MODE_ID);
            assert_eq!(
                option.category,
                Some(SessionConfigOptionCategory::ModelConfig)
            );
            let wire = serde_json::to_value(option).unwrap();
            json!([wire["type"], wire["currentValue"]])
        });
        assert_eq!(advertised, expected, "{fast_mode:?} as {toggles:?}");
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
