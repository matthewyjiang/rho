use pretty_assertions::assert_eq;

use super::SignInTarget;

// Covers: /login claude-code, cursor, and cursor-agent route to their external
// runtimes case-insensitively instead of falling through as a provider id
// Owner: login routing
#[test]
fn sign_in_target_routes_external_runtimes_case_insensitively() {
    fn route(target: SignInTarget) -> String {
        match target {
            SignInTarget::ClaudeCode => "claude-code".into(),
            SignInTarget::Cursor => "cursor".into(),
            SignInTarget::NewCustomHost { .. } => "new-custom-host".into(),
            SignInTarget::Provider(provider) => format!("provider:{provider}"),
        }
    }
    for (value, expected) in [
        ("claude-code", "claude-code"),
        (" Claude-Code ", "claude-code"),
        ("cursor", "cursor"),
        (" Cursor ", "cursor"),
        ("cursor-agent", "cursor"),
        ("CURSOR-AGENT", "cursor"),
        (" anthropic ", "provider:anthropic"),
    ] {
        assert_eq!(route(SignInTarget::parse(value)), expected, "{value:?}");
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
