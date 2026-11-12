use rho_providers::model::catalog::LoginTarget;

use crate::tui::tests::test_app;

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

// Covers: /login must not persist a foreign auth id next to the default provider
// Owner: login
#[test]
fn keyed_custom_login_does_not_write_auth_for_a_different_default_provider() {
    let mut app = test_app();
    let before = app.info.services.config_repository.load().unwrap();

    app.persist_default_login_auth(&LoginTarget {
        provider: "composer".into(),
        auth: "composer-api-key".into(),
        label: "composer API key".into(),
    });

    let saved = app.info.services.config_repository.load().unwrap();
    pretty_assertions::assert_eq!(saved.provider, before.provider);
    pretty_assertions::assert_eq!(saved.auth, before.auth);
}

// Covers: the store picker defaults to the first available backend, and keys
// (arrows, digits, letter shortcuts) never select an unavailable one.
// Owner: pure unit (credential store choice)
#[test]
fn credential_store_choice_selects_only_available_backends() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rho_providers::credentials::{CredentialStoreBackend, CredentialStoreProbe};
    use CredentialStoreBackend::{File, Os};

    use super::{credential_store_inline_choice, selected_credential_store_backend};
    use crate::credential_store::StoreChoiceRequest;

    let cases = [
        (
            "os unavailable",
            false,
            File,
            vec![
                (KeyCode::Up, File),
                (KeyCode::Down, File),
                (KeyCode::Char('1'), File),
                (KeyCode::Char('2'), File),
                (KeyCode::Char('f'), File),
            ],
        ),
        (
            "os available",
            true,
            Os,
            vec![(KeyCode::Down, File), (KeyCode::Char('o'), Os)],
        ),
    ];
    for (case, os_available, initial, steps) in cases {
        let request = StoreChoiceRequest {
            os: CredentialStoreProbe {
                backend: Os,
                available: os_available,
                detail: "os".into(),
            },
            file: CredentialStoreProbe {
                backend: File,
                available: true,
                detail: "ok".into(),
            },
        };
        let mut choice = credential_store_inline_choice(request).expect("file available");
        assert_eq!(
            selected_credential_store_backend(&choice),
            initial,
            "{case}: initial"
        );
        for (key, expected) in steps {
            choice.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
            assert_eq!(
                selected_credential_store_backend(&choice),
                expected,
                "{case}: after {key:?}"
            );
        }
    }
}
