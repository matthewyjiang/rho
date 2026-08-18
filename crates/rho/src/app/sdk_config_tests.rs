use std::path::Path;

use super::SdkBootstrapOptions;
use pretty_assertions::assert_eq;
use {crate::config::Config, rho_providers::providers::ProviderBuildOptions};

// Covers: configured Ollama base URL must reach provider build options
// Owner: sdk config bootstrap
#[test]
fn passes_configured_ollama_base_to_provider_build_options() {
    let mut config = Config {
        provider: "ollama".into(),
        model: "local-model".into(),
        auth: "none".into(),
        ..Config::default()
    };
    config
        .providers
        .set_endpoint("ollama", "http://ollama.internal:22000/v1")
        .unwrap();

    let actual = SdkBootstrapOptions::from_config(&config, Path::new("workspace")).unwrap();
    let expected = ProviderBuildOptions::new("ollama", "local-model", config.reasoning)
        .unwrap()
        .hosted_web_search(/*enabled*/ false)
        .endpoint(
            config
                .providers
                .ollama
                .as_ref()
                .expect("ollama endpoint")
                .base_url
                .clone(),
        )
        .unwrap();

    assert_eq!(actual.provider, expected);
}
