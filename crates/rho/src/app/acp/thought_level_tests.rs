use agent_client_protocol::{
    schema::v1::{
        SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
        SessionConfigOptionValue, SessionConfigSelectOptions, SessionId,
        SetSessionConfigOptionRequest,
    },
    ErrorCode,
};
use pretty_assertions::assert_eq;
use rho_protocol as wire;

use super::{config_options, parse_thought_level_request, THOUGHT_LEVEL_ID};

fn select_current(options: &[SessionConfigOption]) -> &str {
    match &options[0].kind {
        SessionConfigKind::Select(select) => select.current_value.0.as_ref(),
        SessionConfigKind::Boolean(_) => panic!("thought_level must be a select option"),
        _ => panic!("unexpected session config kind"),
    }
}

/// `(value id, label)` pairs in advertised order.
fn select_entries(options: &[SessionConfigOption]) -> Vec<(String, String)> {
    match &options[0].kind {
        SessionConfigKind::Select(select) => match &select.options {
            SessionConfigSelectOptions::Ungrouped(values) => values
                .iter()
                .map(|option| (option.value.0.as_ref().to_string(), option.name.clone()))
                .collect(),
            _ => panic!("thought_level must be ungrouped"),
        },
        _ => panic!("thought_level must be a select option"),
    }
}

// Covers: session/new and session/load must advertise thought_level as the
// current reasoning select, not omit it or encode it as a boolean, with
// lowercase config ids hosts send back and capitalized labels hosts display; a
// model with no configurable reasoning omits it so hosts do not render a
// one-entry select that can never change, nor a level it cannot send back.
// Owner: acp session config mapper
#[test]
fn advertises_thought_level_select_for_the_current_reasoning() {
    // A level a newer daemon sends is left out.
    let reasoning = wire::ReasoningOptions {
        current: wire::ReasoningLevel::High,
        available: vec![
            wire::ReasoningLevel::Off,
            wire::ReasoningLevel::Minimal,
            wire::ReasoningLevel::Low,
            wire::ReasoningLevel::Medium,
            wire::ReasoningLevel::High,
            wire::ReasoningLevel::Xhigh,
            wire::ReasoningLevel::Max,
            wire::ReasoningLevel::Unknown,
        ],
    };
    let options = config_options(Some(&reasoning));
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].id.0.as_ref(), THOUGHT_LEVEL_ID);
    assert_eq!(
        options[0].category,
        Some(SessionConfigOptionCategory::ThoughtLevel)
    );
    assert_eq!(select_current(&options), "high");
    let entries = select_entries(&options);
    assert_eq!(
        entries
            .iter()
            .map(|(value, label)| (value.as_str(), label.as_str()))
            .collect::<Vec<_>>(),
        [
            ("off", "Off"),
            ("minimal", "Minimal"),
            ("low", "Low"),
            ("medium", "Medium"),
            ("high", "High"),
            ("xhigh", "Extra high"),
            ("max", "Max"),
        ]
    );

    assert!(config_options(None).is_empty());
    let unknown_current = wire::ReasoningOptions {
        current: wire::ReasoningLevel::Unknown,
        ..reasoning
    };
    assert!(config_options(Some(&unknown_current)).is_empty());
}

// Covers: hosts must get InvalidParams for a non-select thought_level value
// instead of a silent no-op. Unknown option ids are rejected by the model
// resolver after SessionHost dispatches.
// Owner: acp session config mapper
#[test]
fn parse_thought_level_rejects_unknown_option_and_non_select_values() {
    let session = SessionId::new("sess");
    let cases = [
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
