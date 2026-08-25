use rho_providers::{
    model::{
        catalog::LoginTarget,
        provider_models::{
            replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
            ProviderModel,
        },
        ReasoningCapabilities, ReasoningLevelSet, ReasoningRequestSource,
    },
    reasoning::ReasoningLevel,
};

use crate::{config::Config, tui::tests::test_app};

use super::super::custom_provider_login::CustomHostStep;

// Covers: blank optional key must keep a reachable key
// Owner: login policy
#[test]
fn blank_optional_key_keeps_a_reachable_key_and_runs_keyless_when_none_exists() {
    let keyed = LoginTarget {
        provider: "ollama".into(),
        auth: "ollama-api-key".into(),
        label: "Ollama API key".into(),
    };

    pretty_assertions::assert_eq!(
        super::resolve_blank_optional_key(keyed.clone(), true),
        keyed
    );
    pretty_assertions::assert_eq!(
        super::resolve_blank_optional_key(keyed, false),
        LoginTarget {
            provider: "ollama".into(),
            auth: "none".into(),
            label: "ollama".into(),
        }
    );
}

#[test]
fn login_state_save_persists_reasoning_and_normalizes_auth_profile() {
    let mut app = test_app();
    assert_ne!(
        app.info
            .services
            .config_repository
            .configured_path()
            .unwrap(),
        Config::default_path().unwrap()
    );
    app.info.runtime.provider = "kimi-code".into();
    app.info.runtime.model = "login-k3-test".into();
    app.info.runtime.auth = "api-key".into();
    app.info.runtime.reasoning = ReasoningLevel::High;

    app.save_current_config().unwrap();

    let saved = app.info.services.config_repository.load().unwrap();
    assert_eq!(saved.provider, "kimi-code");
    assert_eq!(saved.model, "login-k3-test");
    assert_eq!(saved.auth, "kimi-oauth");
    assert_eq!(saved.reasoning, ReasoningLevel::High);
}

// Covers: choosing Responses in /login must persist api, not default chat
// Owner: login
#[test]
fn custom_onboarding_persists_selected_responses_api() {
    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();

    let mut app = test_app();
    let api = rho_providers::provider::OpenAiCompatibleApi::Responses;
    app.start_custom_provider_onboarding(api);
    app.submit_custom_host_step(CustomHostStep::Name { api }, "litellm".into())
        .unwrap();
    app.submit_custom_host_step(
        CustomHostStep::CustomUrl {
            name: "litellm".into(),
            api,
        },
        "http://127.0.0.1:4000/v1".into(),
    )
    .unwrap();

    let saved = app.info.services.config_repository.load().unwrap();
    pretty_assertions::assert_eq!(
        saved.providers.custom["litellm"].api,
        rho_providers::provider::OpenAiCompatibleApi::Responses
    );
    pretty_assertions::assert_eq!(
        rho_providers::provider::interned_custom_provider("litellm")
            .unwrap()
            .openai_compatible_api(),
        rho_providers::provider::OpenAiCompatibleApi::Responses
    );
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
}

// Covers: /login on an active custom host must persist `{name}-api-key`
// Owner: login
#[test]
fn keyed_custom_login_persists_auth_mode_for_active_provider() {
    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();

    let mut app = test_app();
    app.info.runtime.provider = "composer".into();
    app.info.runtime.model = "composer-2.5".into();
    app.info.runtime.auth = "none".into();
    app.info
        .services
        .config_repository
        .update(|config| {
            config
                .providers
                .set_endpoint("composer", "http://127.0.0.1:8787/v1")
                .unwrap();
            config.provider = "composer".into();
            config.model = "composer-2.5".into();
            config.auth = "none".into();
        })
        .unwrap();

    app.persist_login_auth(&LoginTarget {
        provider: "composer".into(),
        auth: "composer-api-key".into(),
        label: "composer API key".into(),
    });

    assert_eq!(app.info.runtime.auth, "composer-api-key");
    let saved = app.info.services.config_repository.load().unwrap();
    assert_eq!(saved.auth, "composer-api-key");
}

// Covers: /login must not persist a foreign auth id next to the current provider
// Owner: login
#[test]
fn keyed_custom_login_does_not_write_auth_for_a_different_runtime_provider() {
    let mut app = test_app();
    app.using_unavailable_provider = true;
    let before = app.info.services.config_repository.load().unwrap();

    app.persist_login_auth(&LoginTarget {
        provider: "composer".into(),
        auth: "composer-api-key".into(),
        label: "composer API key".into(),
    });

    pretty_assertions::assert_eq!(app.info.runtime.provider.as_str(), "openai");
    pretty_assertions::assert_eq!(app.info.runtime.auth.as_str(), "api-key");
    let saved = app.info.services.config_repository.load().unwrap();
    pretty_assertions::assert_eq!(saved.provider, before.provider);
    pretty_assertions::assert_eq!(saved.auth, before.auth);
}

#[test]
fn refreshed_login_capabilities_reject_explicit_and_normalize_persisted_reasoning() {
    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), || {
        replace_cached_provider_models_for_tests(
            "kimi-code",
            &[ProviderModel {
                provider: "kimi-code".into(),
                model: "login-k3-test".into(),
                display_name: "Login K3 Test".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Levels(ReasoningLevelSet::new(
                    vec![
                        ReasoningLevel::Off,
                        ReasoningLevel::Low,
                        ReasoningLevel::High,
                        ReasoningLevel::Max,
                    ],
                )),
            }],
        )
        .unwrap();
        let mut app = test_app();
        app.info.runtime.reasoning = ReasoningLevel::Medium;
        app.info.runtime.reasoning_source = ReasoningRequestSource::Explicit;

        assert!(app
            .resolve_reasoning_after_login("kimi-code", "login-k3-test")
            .is_none());

        app.info.runtime.reasoning_source = ReasoningRequestSource::PersistedOrDefault;
        let resolved = app
            .resolve_reasoning_after_login("kimi-code", "login-k3-test")
            .unwrap();
        assert_eq!(resolved.effective, ReasoningLevel::High);
        assert_eq!(resolved.source, ReasoningRequestSource::PersistedOrDefault);
    });
}

#[test]
fn first_login_preserves_explicit_reasoning_when_capabilities_are_unknown() {
    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), || {
        let mut app = test_app();
        app.info.runtime.reasoning = ReasoningLevel::Off;
        app.info.runtime.reasoning_source = ReasoningRequestSource::Explicit;

        let resolved = app
            .resolve_reasoning_after_login("kimi-code", "unknown-login-model")
            .unwrap();
        assert_eq!(resolved.effective, ReasoningLevel::Off);
        assert_eq!(resolved.source, ReasoningRequestSource::Explicit);
    });
}

#[test]
fn credential_store_choice_defaults_to_first_available_and_skips_unavailable() {
    use rho_providers::credentials::{CredentialStoreBackend, CredentialStoreProbe};

    use super::{credential_store_inline_choice, selected_credential_store_backend};
    use crate::credential_store::StoreChoiceRequest;

    let request = StoreChoiceRequest {
        os: CredentialStoreProbe {
            backend: CredentialStoreBackend::Os,
            available: false,
            detail: "no keyring".into(),
        },
        file: CredentialStoreProbe {
            backend: CredentialStoreBackend::File,
            available: true,
            detail: "ok".into(),
        },
    };
    let mut choice = credential_store_inline_choice(request).expect("file available");
    assert_eq!(
        selected_credential_store_backend(&choice),
        CredentialStoreBackend::File,
        "default should land on first available backend"
    );

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    choice.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(
        selected_credential_store_backend(&choice),
        CredentialStoreBackend::File,
        "navigation must not land on unavailable OS backend"
    );
    choice.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        selected_credential_store_backend(&choice),
        CredentialStoreBackend::File
    );

    choice.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(
        selected_credential_store_backend(&choice),
        CredentialStoreBackend::File
    );
    choice.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(
        selected_credential_store_backend(&choice),
        CredentialStoreBackend::File
    );
    choice.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    assert_eq!(
        selected_credential_store_backend(&choice),
        CredentialStoreBackend::File
    );
}

#[test]
fn credential_store_choice_os_shortcut_when_available() {
    use rho_providers::credentials::{CredentialStoreBackend, CredentialStoreProbe};

    use super::{credential_store_inline_choice, selected_credential_store_backend};
    use crate::credential_store::StoreChoiceRequest;

    let request = StoreChoiceRequest {
        os: CredentialStoreProbe {
            backend: CredentialStoreBackend::Os,
            available: true,
            detail: "ok".into(),
        },
        file: CredentialStoreProbe {
            backend: CredentialStoreBackend::File,
            available: true,
            detail: "ok".into(),
        },
    };
    let mut choice = credential_store_inline_choice(request).expect("backends");
    assert_eq!(
        selected_credential_store_backend(&choice),
        CredentialStoreBackend::Os
    );
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    choice.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        selected_credential_store_backend(&choice),
        CredentialStoreBackend::File
    );
    choice.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    assert_eq!(
        selected_credential_store_backend(&choice),
        CredentialStoreBackend::Os
    );
}
