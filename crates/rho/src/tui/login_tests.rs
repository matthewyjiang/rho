use rho_providers::{
    model::{
        provider_models::{
            replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
            ProviderModel,
        },
        ReasoningCapabilities, ReasoningLevelSet, ReasoningRequestSource,
    },
    reasoning::ReasoningLevel,
};

use crate::{config::Config, tui::tests::test_app};

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

    use super::{CredentialStoreChoice, StoreChoiceNext};
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
    let mut choice =
        CredentialStoreChoice::new(request, StoreChoiceNext::OpenPicker).expect("file available");
    assert_eq!(
        choice.selected_backend(),
        CredentialStoreBackend::File,
        "default should land on first available backend"
    );

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    choice
        .choice
        .handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(
        choice.selected_backend(),
        CredentialStoreBackend::File,
        "navigation must not land on unavailable OS backend"
    );
    choice
        .choice
        .handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(choice.selected_backend(), CredentialStoreBackend::File);

    choice
        .choice
        .handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(choice.selected_backend(), CredentialStoreBackend::File);
    choice
        .choice
        .handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(choice.selected_backend(), CredentialStoreBackend::File);
    choice
        .choice
        .handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    assert_eq!(choice.selected_backend(), CredentialStoreBackend::File);
}

#[test]
fn credential_store_choice_os_shortcut_when_available() {
    use rho_providers::credentials::{CredentialStoreBackend, CredentialStoreProbe};

    use super::{CredentialStoreChoice, StoreChoiceNext};
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
    let mut choice =
        CredentialStoreChoice::new(request, StoreChoiceNext::OpenPicker).expect("backends");
    assert_eq!(choice.selected_backend(), CredentialStoreBackend::Os);
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    choice
        .choice
        .handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(choice.selected_backend(), CredentialStoreBackend::File);
    choice
        .choice
        .handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    assert_eq!(choice.selected_backend(), CredentialStoreBackend::Os);
}
