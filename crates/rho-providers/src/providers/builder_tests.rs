use std::time::Duration;

use rho_sdk::SecretString;
use url::Url;

use super::{ProviderBuildOptions, ProviderBuilder, ProviderCredential};
use crate::{providers::openai_compatible::CompatibleAuth, reasoning::ReasoningLevel};

#[test]
fn options_reject_invalid_states_and_accept_typed_overrides() {
    assert!(ProviderBuildOptions::new("", "model", ReasoningLevel::Off).is_err());
    assert!(ProviderBuildOptions::new("openai", "", ReasoningLevel::Off).is_err());
    assert!(ProviderBuildOptions::new("unknown", "model", ReasoningLevel::Off).is_err());

    let options = ProviderBuildOptions::new("openai", "model", ReasoningLevel::Low)
        .unwrap()
        .endpoint(Url::parse("https://example.test/v1").unwrap())
        .unwrap()
        .request_timeout(Duration::from_secs(30))
        .unwrap();

    assert_eq!(options.provider(), "openai");
    assert_eq!(options.model(), "model");
    assert!(
        ProviderBuildOptions::new("openai", "model", ReasoningLevel::Off)
            .unwrap()
            .endpoint(Url::parse("file:///tmp/provider").unwrap())
            .is_err()
    );
    assert!(
        ProviderBuildOptions::new("openai", "model", ReasoningLevel::Off)
            .unwrap()
            .request_timeout(Duration::ZERO)
            .is_err()
    );
}

#[test]
fn credentials_are_redacted_and_mismatches_fail_before_execution() {
    let secret = "sk-provider-secret";
    let credential = ProviderCredential::AnthropicApiKey(SecretString::new(secret));
    let debug = format!("{credential:?}");
    assert!(debug.contains("anthropic-api-key"));
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(secret));

    let options = ProviderBuildOptions::new("openai", "gpt-test", ReasoningLevel::Off).unwrap();
    let error = match ProviderBuilder::new(options, credential).build() {
        Ok(_) => panic!("mismatched credential unexpectedly built a provider"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("credential kind does not match provider"));
    assert!(!format!("{error:?}").contains(secret));

    let options = ProviderBuildOptions::new("ollama", "local-model", ReasoningLevel::Off).unwrap();
    let error = match ProviderBuilder::new(
        options,
        ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey(secret.into())),
    )
    .build()
    {
        Ok(_) => panic!("authenticated Ollama credential unexpectedly built a provider"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("credential kind does not match provider"));
    assert!(!format!("{error:?}").contains(secret));
}

// Covers: custom-host construction must not depend on a still-live thread scope
// Owner: provider builder
#[test]
fn custom_host_build_survives_dropped_thread_scope() {
    let _lock = crate::provider::custom_provider_registry_test_lock();
    crate::provider::reset_custom_openai_compatible_providers_for_tests();
    struct RestoreCustomProviders;
    impl Drop for RestoreCustomProviders {
        fn drop(&mut self) {
            crate::provider::reset_custom_openai_compatible_providers_for_tests();
        }
    }
    let _restore = RestoreCustomProviders;

    let names = crate::provider::intern_custom_openai_compatible_providers(["composer"]).unwrap();
    let options = {
        let _scope = crate::provider::CustomProviderThreadScope::enter(names);
        ProviderBuildOptions::new("composer", "local-model", ReasoningLevel::Off)
            .unwrap()
            .endpoint(Url::parse("http://127.0.0.1:8787/v1").unwrap())
            .unwrap()
    };
    assert!(
        crate::provider::provider_descriptor("composer").is_none(),
        "scope drop must hide the name from process-wide lookup"
    );
    ProviderBuilder::new(
        options,
        ProviderCredential::OpenAiCompatible(CompatibleAuth::None),
    )
    .build()
    .expect("interned custom host must still build after the constructing scope drops");
}
