use pretty_assertions::assert_eq;

use super::SignInTarget;

#[test]
fn sign_in_target_routes_claude_code_case_insensitively() {
    for value in ["claude-code", " Claude-Code "] {
        assert!(
            matches!(SignInTarget::parse(value), SignInTarget::ClaudeCode),
            "{value} must route to the external runtime"
        );
    }
    assert!(matches!(
        SignInTarget::parse(" anthropic "),
        SignInTarget::Provider(provider) if provider == "anthropic"
    ));
}

// Covers: /login cursor and cursor-agent must not fall through as a provider id
// Owner: login routing
#[test]
fn sign_in_target_parses_cursor_and_alias() {
    for (value, expected_cursor) in [
        ("cursor", true),
        (" Cursor ", true),
        ("cursor-agent", true),
        ("CURSOR-AGENT", true),
        ("claude-code", false),
        ("anthropic", false),
    ] {
        assert_eq!(
            matches!(SignInTarget::parse(value), SignInTarget::Cursor),
            expected_cursor,
            "{value}"
        );
    }
}

// Covers: /login picker values must map each custom host API
// Owner: login routing
#[test]
fn sign_in_target_routes_custom_host_api_methods() {
    assert!(matches!(
        SignInTarget::parse(
            super::super::custom_provider_login::NEW_CUSTOM_CHAT_COMPLETIONS_HOST_VALUE
        ),
        SignInTarget::NewCustomHost {
            api: rho_providers::provider::OpenAiCompatibleApi::ChatCompletions
        }
    ));
    assert!(matches!(
        SignInTarget::parse(super::super::custom_provider_login::NEW_CUSTOM_RESPONSES_HOST_VALUE),
        SignInTarget::NewCustomHost {
            api: rho_providers::provider::OpenAiCompatibleApi::Responses
        }
    ));
}
