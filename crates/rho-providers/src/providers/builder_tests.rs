use std::{sync::Arc, time::Duration};

use rho_sdk::SecretString;
use url::Url;

use super::{ProviderBuildOptions, ProviderBuilder, ProviderCredential};
use crate::{
    credentials::MemoryCredentialStore,
    providers::{openai::auth::Auth, openai_compatible::CompatibleAuth},
    reasoning::ReasoningLevel,
};

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
}

#[test]
fn explicit_builder_constructs_provider_without_environment_or_keychain_lookup() {
    let options = ProviderBuildOptions::new("openai", "gpt-test", ReasoningLevel::Medium).unwrap();
    let credential = ProviderCredential::OpenAi {
        auth: Auth::ApiKey("explicit-secret".into()),
        refresh_store: Arc::new(MemoryCredentialStore::default()),
    };

    let provider = ProviderBuilder::new(options, credential).build().unwrap();

    assert_eq!(provider.identity().provider, "openai");
    assert_eq!(provider.identity().model, "gpt-test");
}

#[test]
fn explicit_builder_constructs_google_provider() {
    let options =
        ProviderBuildOptions::new("google", "gemini-3.5-flash", ReasoningLevel::Medium).unwrap();
    let credential = ProviderCredential::GoogleApiKey(SecretString::new("explicit-secret"));

    let provider = ProviderBuilder::new(options, credential).build().unwrap();

    assert_eq!(provider.identity().provider, "google");
    assert_eq!(provider.identity().api, "gemini-generate-content");
    assert_eq!(provider.identity().model, "gemini-3.5-flash");
}

#[test]
fn explicit_builder_constructs_openrouter_provider() {
    let options = ProviderBuildOptions::new(
        "openrouter",
        "anthropic/claude-sonnet-4",
        ReasoningLevel::Medium,
    )
    .unwrap();
    let credential =
        ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey("explicit-secret".into()));

    let provider = ProviderBuilder::new(options, credential).build().unwrap();

    assert_eq!(provider.identity().provider, "openrouter");
    assert_eq!(provider.identity().model, "anthropic/claude-sonnet-4");
}

#[test]
fn public_provider_objects_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<dyn rho_sdk::provider::ModelProvider>>();
}
