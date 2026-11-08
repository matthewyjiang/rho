use agent_client_protocol::{
    schema::v1::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption, SessionId,
        SetSessionConfigOptionRequest,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use rho_protocol as wire;

use super::{model_config_option, resolve_model_value, CurrentModel, MODEL_CONFIG_ID};

fn model_ref(provider: &str, model: &str) -> wire::ModelRef {
    wire::ModelRef {
        provider: provider.into(),
        model: model.into(),
    }
}

// Covers: the ACP model select keeps the session's model order and short
// labels, with `provider/model` value ids hosts send back and the same id
// as each option's description, since host pickers truncate labels
// Owner: ACP config options
#[test]
fn model_option_carries_provider_model_ids_in_order() {
    let models = wire::ModelOptions {
        current: model_ref("openai", "gpt-4"),
        available: vec![
            wire::ModelChoice {
                model: model_ref("xai", "grok-3"),
                label: "Grok 3".into(),
            },
            wire::ModelChoice {
                model: model_ref("openai", "gpt-4"),
                label: "gpt-4".into(),
            },
        ],
    };

    let expected = SessionConfigOption::select(
        MODEL_CONFIG_ID,
        "Model",
        "openai/gpt-4",
        vec![
            SessionConfigSelectOption::new("xai/grok-3", "Grok 3").description("xai/grok-3"),
            SessionConfigSelectOption::new("openai/gpt-4", "gpt-4").description("openai/gpt-4"),
        ],
    )
    .category(SessionConfigOptionCategory::Model);
    assert_eq!(model_config_option(&models), expected);
}

// Covers: session/set_config_option must reject what only ACP parses: an
// unknown option id, a boolean value, and a value without a provider
// (`session_options` refuses unknown models)
// Owner: ACP config options
#[test]
fn resolve_model_value_rejects_invalid_requests() {
    let current = CurrentModel {
        provider: "xai".into(),
        model: "grok-3".into(),
        auth: "api-key".into(),
    };
    let session = SessionId::new("session");
    let cases = [
        (
            SetSessionConfigOptionRequest::new(session.clone(), "mode", "xai/grok-3"),
            AcpError::invalid_params().data("unknown config option 'mode'"),
        ),
        (
            SetSessionConfigOptionRequest::new(session.clone(), MODEL_CONFIG_ID, true),
            AcpError::invalid_params().data("model option requires a select value"),
        ),
        (
            SetSessionConfigOptionRequest::new(session, MODEL_CONFIG_ID, "grok-3"),
            AcpError::invalid_params().data("invalid model value 'grok-3'"),
        ),
    ];

    for (request, expected) in cases {
        assert_eq!(
            resolve_model_value(&request, &current, &["xai-api-key".to_string()]),
            Err(expected),
            "{:?}",
            request.value
        );
    }
}
