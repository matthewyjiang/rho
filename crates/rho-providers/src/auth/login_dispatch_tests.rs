use super::{
    interactive_login, AuthenticationError, AuthenticationMethod, InteractiveLoginCompletion,
    ProviderAuthentication, StartedLogin,
};
use crate::{
    auth::{
        browser::{BrowserAvailability, BrowserOpen},
        login_prompt::LoginPrompt,
    },
    credentials::MemoryCredentialStore,
};

#[test]
fn dispatches_registered_providers_to_typed_authentication_methods() {
    assert_eq!(
        ProviderAuthentication::method("openai").unwrap(),
        AuthenticationMethod::ApiKey {
            entry_label: "OpenAI API key",
        }
    );
    assert_eq!(
        ProviderAuthentication::method("openai-codex").unwrap(),
        AuthenticationMethod::Interactive {
            provider_label: "Codex",
        }
    );
    assert_eq!(
        ProviderAuthentication::method("github-copilot").unwrap(),
        AuthenticationMethod::Interactive {
            provider_label: "GitHub Copilot",
        }
    );
    assert_eq!(
        ProviderAuthentication::method("kimi-code").unwrap(),
        AuthenticationMethod::Interactive {
            provider_label: "Kimi",
        }
    );
    assert_eq!(
        ProviderAuthentication::method("openrouter-oauth").unwrap(),
        AuthenticationMethod::Interactive {
            provider_label: "OpenRouter",
        }
    );
    assert_eq!(
        ProviderAuthentication::method("xai-oauth").unwrap(),
        AuthenticationMethod::Interactive {
            provider_label: "xAI",
        }
    );
    assert_eq!(
        ProviderAuthentication::method("ollama-cloud-device").unwrap(),
        AuthenticationMethod::Interactive {
            provider_label: "Ollama Cloud",
        }
    );
    assert!(ProviderAuthentication::supports_device_login("xai-oauth"));
    assert!(ProviderAuthentication::supports_device_login(
        "ollama-cloud-device"
    ));
    assert!(!ProviderAuthentication::supports_device_login(
        "openrouter-oauth"
    ));
}

// Covers: headless prefers device when the provider has one
// Owner: login dispatch
#[test]
fn preferred_mode_uses_device_only_when_headless_and_capable() {
    let cases = [
        (
            "openai-codex",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Device,
        ),
        (
            "openai-codex",
            BrowserAvailability::Graphical,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "github-copilot",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "github-copilot",
            BrowserAvailability::Graphical,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "xai-oauth",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Device,
        ),
        (
            "kimi-code",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "ollama-cloud-device",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "openrouter-oauth",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "openrouter-oauth",
            BrowserAvailability::Graphical,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "openai",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Browser,
        ),
    ];
    for (provider, availability, expected) in cases {
        pretty_assertions::assert_eq!(
            ProviderAuthentication::preferred_mode(provider, availability),
            expected,
            "{provider} {availability:?}"
        );
    }
}

#[test]
fn ollama_device_setup_does_not_wait_for_confirmation() {
    let login = interactive_login(
        StartedLogin {
            provider_label: "Ollama Cloud",
            prompt: LoginPrompt::browser_flow(
                "https://ollama.com/connect?key=test",
                "Open this URL and approve the device for Ollama Cloud.",
            ),
            completion: InteractiveLoginCompletion::Unconfirmed {
                instruction: "Approve the device in your browser, then use an Ollama Cloud model. Rho does not receive a completion callback.",
            },
        },
        BrowserAvailability::Headless,
    );

    pretty_assertions::assert_eq!(login.provider_label, "Ollama Cloud");
    pretty_assertions::assert_eq!(
        login.prompt,
        LoginPrompt::browser_flow(
            "https://ollama.com/connect?key=test",
            "Open this URL and approve the device for Ollama Cloud.",
        )
    );
    assert!(matches!(
        login.completion,
        InteractiveLoginCompletion::Unconfirmed { .. }
    ));
}

#[test]
fn multi_auth_provider_name_is_ambiguous_for_login() {
    let error = ProviderAuthentication::method("ollama-cloud").unwrap_err();
    match error {
        AuthenticationError::AmbiguousProvider { provider, auth_ids } => {
            assert_eq!(provider, "ollama-cloud");
            assert!(auth_ids.contains(&"ollama-cloud-api-key"));
            assert!(auth_ids.contains(&"ollama-cloud-device"));
        }
        other => panic!("expected AmbiguousProvider, got {other:?}"),
    }
}

#[test]
fn owns_api_key_storage_and_deletion() {
    let store = MemoryCredentialStore::default();

    ProviderAuthentication::save_api_key(&store, "openai", "sk-test").unwrap();
    assert!(ProviderAuthentication::has_credentials(&store, "openai").unwrap());
    assert!(ProviderAuthentication::has_stored_credentials(&store, "openai").unwrap());
    assert!(ProviderAuthentication::delete_credentials(&store, "openai").unwrap());
    assert!(!ProviderAuthentication::has_credentials(&store, "openai").unwrap());
}

#[test]
fn rejects_unknown_provider_before_starting_authentication() {
    assert!(matches!(
        ProviderAuthentication::method("missing"),
        Err(AuthenticationError::UnsupportedProvider(provider)) if provider == "missing"
    ));
}

// Covers: LoginPrompt maps onto the deprecated InteractiveUserAction shim without drift
// Owner: login dispatch
#[allow(deprecated)]
#[test]
fn login_prompt_maps_to_interactive_user_action() {
    let cases = [
        (
            "device code",
            LoginPrompt::device_code(
                "https://auth.example/device",
                "WD4E-T6MC",
                Some("https://auth.example/device?user_code=WD4E-T6MC".into()),
                "Visit this URL and enter the code.",
            )
            .with_browser(BrowserOpen::Launched),
            super::InteractiveUserAction::DeviceCode {
                verification_uri: "https://auth.example/device".into(),
                user_code: "WD4E-T6MC".into(),
                verification_uri_complete: Some(
                    "https://auth.example/device?user_code=WD4E-T6MC".into(),
                ),
            },
        ),
        (
            "launched browser",
            LoginPrompt::browser_flow(
                "https://auth.example/authorize",
                "Open this URL to finish login.",
            )
            .with_browser(BrowserOpen::Launched),
            super::InteractiveUserAction::BrowserOpened,
        ),
        (
            "not launched",
            LoginPrompt::browser_flow(
                "https://auth.example/authorize",
                "Open this URL to finish login.",
            ),
            super::InteractiveUserAction::OpenUrl {
                url: "https://auth.example/authorize".into(),
                instruction: "Open this URL to finish login.".into(),
            },
        ),
    ];
    for (name, prompt, expected) in cases {
        pretty_assertions::assert_eq!(
            super::InteractiveUserAction::from(&prompt),
            expected,
            "{name}"
        );
    }
}
