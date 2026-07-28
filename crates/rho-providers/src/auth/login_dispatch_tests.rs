use super::{AuthenticationError, AuthenticationMethod, ProviderAuthentication};
use crate::credentials::MemoryCredentialStore;

#[test]
fn dispatches_registered_providers_to_typed_authentication_methods() {
    assert_eq!(
        ProviderAuthentication::method("openai").unwrap(),
        AuthenticationMethod::ApiKey {
            entry_label: "OpenAI API key"
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
