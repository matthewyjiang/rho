use pretty_assertions::assert_eq;
use rho_providers::model::ModelError;

use super::{is_interactive_startup_unavailable_error, parse_first_run_override, SetupEntry};

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

/// The override has to be able to name a step. A configured machine already
/// lists models, so an on/off flag would always land on the model step and
/// leave the provider menu unreachable.
#[test]
fn the_first_run_override_selects_a_setup_step() {
    let cases = [
        ("", None),
        ("0", None),
        ("false", None),
        ("no", None),
        ("signin", Some(SetupEntry::SignIn)),
        ("sign-in", Some(SetupEntry::SignIn)),
        ("login", Some(SetupEntry::SignIn)),
        ("  SignIn  ", Some(SetupEntry::SignIn)),
        ("model", Some(SetupEntry::ChooseModel)),
        ("models", Some(SetupEntry::ChooseModel)),
        ("MODEL", Some(SetupEntry::ChooseModel)),
        ("1", Some(SetupEntry::Auto)),
        ("true", Some(SetupEntry::Auto)),
        ("yes please", Some(SetupEntry::Auto)),
    ];

    for (value, expected) in cases {
        assert_eq!(
            parse_first_run_override(value),
            expected,
            "override value {value:?}"
        );
    }
}
