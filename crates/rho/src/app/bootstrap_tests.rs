use rho_providers::model::ModelError;

use super::is_interactive_startup_unavailable_error;

#[test]
fn missing_xai_api_key_is_nonfatal_for_interactive_startup() {
    assert!(is_interactive_startup_unavailable_error(
        &ModelError::missing_credentials("missing xAI API key; run /login xai in the TUI or set XAI_API_KEY as a CI/dev override")
    ));
}

#[test]
fn unsupported_provider_is_nonfatal_for_interactive_startup() {
    assert!(is_interactive_startup_unavailable_error(
        &ModelError::UnsupportedProvider("anthropic".into())
    ));
}
