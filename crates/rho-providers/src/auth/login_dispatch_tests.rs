use std::sync::Arc;

use super::{AuthenticationError, ProviderAuthentication};
use crate::{
    auth::{
        browser::{BrowserAvailability, BrowserOpen},
        login_prompt::LoginPrompt,
        ollama_device::{with_ollama_device_key_dir_for_tests, OllamaDeviceKey},
        provider_credentials::{ApplicationCredentialSource, ProviderCredentialSource},
    },
    credentials::{available_auth_modes, MemoryCredentialStore},
};

// Covers: public device-login capability matches the providers with a device grant
// Owner: login dispatch
#[test]
fn supports_device_login_only_for_device_capable_providers() {
    for (provider, expected) in [
        ("xai-oauth", true),
        ("ollama-cloud-device", true),
        ("openrouter-oauth", false),
    ] {
        assert_eq!(
            ProviderAuthentication::supports_device_login(provider),
            expected,
            "{provider}"
        );
    }
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

// Covers: login method lookup refuses an ambiguous or unknown provider instead
// of guessing
// Owner: provider login dispatch
#[test]
fn login_method_rejects_ambiguous_and_unknown_providers() {
    assert!(matches!(
        ProviderAuthentication::method("missing"),
        Err(AuthenticationError::UnsupportedProvider(provider)) if provider == "missing"
    ));
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

// Covers: a device key on disk stays unused until a Rho device login, and logout undoes it
// Owner: login dispatch
#[test]
fn ollama_device_key_needs_rho_login_and_logout_undoes_it() {
    let dir = tempfile::tempdir().unwrap();
    OllamaDeviceKey::load_or_create(dir.path()).unwrap();
    with_ollama_device_key_dir_for_tests(dir.path().into(), || {
        let store = Arc::new(MemoryCredentialStore::default());
        let source = ApplicationCredentialSource::new(store.clone());
        let device = "ollama-cloud-device";
        let signed_in = || ProviderAuthentication::has_credentials(store.as_ref(), device).unwrap();
        let listed = || available_auth_modes(store.as_ref()).contains(&device.to_string());
        assert!(!signed_in());
        assert!(!listed());
        assert!(source.acquire("ollama-cloud", device).is_err());

        ProviderAuthentication::accept_unconfirmed_login(store.as_ref(), device).unwrap();
        assert!(signed_in());
        assert!(listed());
        assert!(source.acquire("ollama-cloud", device).is_ok());

        assert!(ProviderAuthentication::delete_credentials(store.as_ref(), device).unwrap());
        assert!(!signed_in());
        assert!(!listed());
        assert!(source.acquire("ollama-cloud", device).is_err());
        assert!(!ProviderAuthentication::delete_credentials(store.as_ref(), device).unwrap());
        // The key belongs to the Ollama CLI too; logout must leave it on disk.
        assert!(OllamaDeviceKey::load_from_dir(dir.path()).is_ok());

        // Provider-level logout covers every mode, including the device key.
        ProviderAuthentication::accept_unconfirmed_login(store.as_ref(), device).unwrap();
        assert!(
            ProviderAuthentication::delete_credentials(store.as_ref(), "ollama-cloud").unwrap()
        );
        assert!(!signed_in());
    });
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

// Covers: one OpenCode console key logs in to Zen and Go, so login offers a
// sibling's stored key; unrelated or unstored profiles offer nothing, and a
// profile with its own key is not offered a replacement
// Owner: login dispatch
#[test]
fn opencode_logins_reuse_a_sibling_stored_key() {
    let store = MemoryCredentialStore::default();
    let reuse_source =
        |auth| ProviderAuthentication::reusable_api_key_source(&store, auth).unwrap();
    assert_eq!(reuse_source("opencode-api-key"), None);

    ProviderAuthentication::save_api_key(&store, "opencode-go-api-key", "console-key").unwrap();

    let go = reuse_source("opencode-api-key").expect("the stored Go key is offered to Zen");
    assert_eq!(go.id, "opencode-go-api-key");
    assert_eq!(reuse_source("opencode-go-api-key"), None);
    assert_eq!(reuse_source("api-key"), None);
    let shared = |auth| ProviderAuthentication::shared_api_key(&store, go, auth).unwrap();
    assert_eq!(shared("opencode-api-key").as_deref(), Some("console-key"));
    assert_eq!(shared("anthropic-api-key"), None);

    ProviderAuthentication::save_api_key(&store, "opencode-api-key", "zen-key").unwrap();
    assert_eq!(reuse_source("opencode-api-key"), None);
}
