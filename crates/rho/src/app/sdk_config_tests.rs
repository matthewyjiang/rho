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
        .async_tools(crate::tools::async_tool_names(&config))
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

// Covers: a custom host's request_compression must reach provider build
// options without disturbing any other transport setting
// Owner: sdk config bootstrap
#[test]
fn passes_custom_request_compression_to_provider_build_options() {
    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"cliproxyapi\"\nmodel = \"local-model\"\n[providers.custom.cliproxyapi]\nbase_url = \"http://node0:8317/v1\"\napi = \"responses\"\nrequest_compression = \"zstd\"\n",
    )
    .unwrap();
    let mut config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    let actual = SdkBootstrapOptions::from_config(&config, Path::new("workspace")).unwrap();
    let compression = config
        .providers
        .custom
        .get_mut("cliproxyapi")
        .and_then(|endpoint| endpoint.request_compression.take())
        .expect("configured compression");
    let expected = SdkBootstrapOptions::from_config(&config, Path::new("workspace"))
        .unwrap()
        .provider
        .request_compression(compression)
        .unwrap();

    assert_eq!(actual.provider, expected);
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
}
