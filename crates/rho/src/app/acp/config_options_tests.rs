use agent_client_protocol::{
    schema::v1::{
        SessionConfigKind, SessionConfigOptionCategory, SessionConfigSelect,
        SessionConfigSelectOptions, SessionId, SetSessionConfigOptionRequest,
    },
    ErrorCode,
};
use pretty_assertions::assert_eq;
use rho_providers::model::catalog::{ModelCatalogEntry, ModelSelection};

use super::{model_config_options, resolve_model_value, CurrentModel, MODEL_CONFIG_ID};

fn entry(provider: &str, model: &str) -> ModelCatalogEntry {
    ModelCatalogEntry {
        provider: provider.into(),
        model: model.into(),
        display_name: model.into(),
        auth_modes: vec!["api-key".into()],
    }
}

fn current(provider: &str, model: &str) -> CurrentModel {
    CurrentModel {
        provider: provider.into(),
        model: model.into(),
        auth: "api-key".into(),
    }
}

fn select_options(
    option: &agent_client_protocol::schema::v1::SessionConfigOption,
) -> &SessionConfigSelect {
    match &option.kind {
        SessionConfigKind::Select(select) => select,
        other => panic!("expected select kind, got {other:?}"),
    }
}

fn option_values(select: &SessionConfigSelect) -> Vec<String> {
    match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .map(|option| option.value.0.as_ref().to_string())
            .collect(),
        _ => panic!("expected an ungrouped model list"),
    }
}

// Covers: ACP model select must follow TUI picker order and provider/model ids
// Owner: ACP config options
#[test]
fn model_options_list_favorites_first_with_provider_model_ids() {
    let options = model_config_options(
        &current("openai", "gpt-4"),
        &["xai/grok-3".into()],
        vec![entry("openai", "gpt-4"), entry("xai", "grok-3")],
    );

    assert_eq!(options.len(), 1);
    let option = &options[0];
    assert_eq!(option.id.0.as_ref(), MODEL_CONFIG_ID);
    assert_eq!(option.name, "Model");
    assert_eq!(option.category, Some(SessionConfigOptionCategory::Model));
    let select = select_options(option);
    assert_eq!(select.current_value.0.as_ref(), "openai/gpt-4");
    assert_eq!(
        option_values(select),
        vec!["xai/grok-3".to_string(), "openai/gpt-4".to_string()]
    );
}

// Covers: current model must remain selectable even when it is off-catalog
// Owner: ACP config options
#[test]
fn current_model_absent_from_available_is_appended() {
    let options = model_config_options(
        &current("custom", "local-model"),
        &[],
        vec![entry("xai", "grok-3")],
    );

    let select = select_options(&options[0]);
    assert_eq!(select.current_value.0.as_ref(), "custom/local-model");
    assert_eq!(
        option_values(select),
        vec!["xai/grok-3".to_string(), "custom/local-model".to_string()]
    );
}

// Covers: session/set_config_option must reject unknown ids, boolean values, and unknown models
// Owner: ACP config options
#[test]
fn resolve_model_value_rejects_invalid_requests() {
    let current = current("xai", "grok-3");
    let available_auths = ["xai-api-key".to_string()];
    let cases = [
        (
            "unknown config id",
            SetSessionConfigOptionRequest::new(SessionId::new("session"), "mode", "xai/grok-3"),
        ),
        (
            "boolean value",
            SetSessionConfigOptionRequest::new(SessionId::new("session"), MODEL_CONFIG_ID, true),
        ),
        (
            "unknown provider",
            SetSessionConfigOptionRequest::new(
                SessionId::new("session"),
                MODEL_CONFIG_ID,
                "no-such-provider/no-such-model",
            ),
        ),
        (
            "unknown model",
            SetSessionConfigOptionRequest::new(
                SessionId::new("session"),
                MODEL_CONFIG_ID,
                "xai/no-such-model",
            ),
        ),
    ];

    for (label, request) in cases {
        let error = resolve_model_value(&request, &current, &available_auths).expect_err(label);
        assert_eq!(error.code, ErrorCode::InvalidParams, "{label}");
    }
}

// Covers: advertised current values, including off-catalog ones, must resolve
// Owner: ACP config options
#[test]
fn resolve_model_value_accepts_current_model() {
    let current = current("custom", "local-model");
    let request = SetSessionConfigOptionRequest::new(
        SessionId::new("session"),
        MODEL_CONFIG_ID,
        "custom/local-model",
    );

    let selection = resolve_model_value(&request, &current, &["xai-api-key".to_string()]).unwrap();
    assert_eq!(
        selection,
        ModelSelection {
            provider: "custom".into(),
            model: "local-model".into(),
            auth: "api-key".into(),
            from_catalog: false,
        }
    );
}
