use super::*;

#[test]
fn config_normalizes_provider_profiles_for_top_level_and_internal_models() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"openrouter-oauth\"\nmodel = \"anthropic/claude-sonnet-4\"\nauth = \"openrouter-api-key\"\n[internal_agents.session-title]\nprovider = \"openrouter\"\nmodel = \"anthropic/claude-sonnet-4\"\nauth = \"openrouter-oauth\"\n",
    )
    .unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(
        (config.provider.as_str(), config.auth.as_str()),
        ("openrouter", "openrouter-oauth")
    );
    let title = config.internal_agent_model("session-title").unwrap();
    assert_eq!(
        (
            title.expect_rho().provider.as_str(),
            title.expect_rho().auth.as_str()
        ),
        ("openrouter", "openrouter-oauth")
    );
}

#[test]
fn config_canonicalizes_legacy_poolside_wire_model_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"poolside\"\nmodel = \"poolside/laguna-m.1\"\nauth = \"poolside-api-key\"\n[internal_agents.session-title]\nprovider = \"poolside\"\nmodel = \"poolside/poolside/laguna-m.1\"\nauth = \"poolside-api-key\"\n",
    )
    .unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(config.provider, "poolside");
    assert_eq!(config.model, "laguna-m.1");
    let title = config.internal_agent_model("session-title").unwrap();
    assert_eq!(title.expect_rho().provider, "poolside");
    assert_eq!(title.expect_rho().model, "laguna-m.1");
}

// Covers: first-run config must not invent a default Ollama endpoint
// Owner: provider config
#[test]
fn default_config_omits_ollama_endpoint_until_login_or_explicit_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::default().write_settings(path.clone()).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(
        !saved.contains("[providers.ollama]"),
        "first-run config must not write a default Ollama endpoint: {saved}"
    );

    let mut config = Config::default();
    config
        .providers
        .set_endpoint("ollama", rho_providers::model::registry::OLLAMA_API_BASE)
        .unwrap();
    config.write_settings(path.clone()).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(
        saved.contains("[providers.ollama]"),
        "explicit Ollama login must persist the endpoint: {saved}"
    );
    assert!(
        saved.contains(&format!(
            "base_url = \"{}\"",
            rho_providers::model::registry::OLLAMA_API_BASE
        )),
        "saved Ollama endpoint must keep the submitted URL: {saved}"
    );
}

// Covers: config load must use one validated endpoint write path
// Owner: provider config
#[test]
fn set_endpoint_updates_supported_providers_and_rejects_others() {
    let mut providers = ProviderConfigs::default();

    providers
        .set_endpoint("ollama", "http://10.0.0.5:11434/v1")
        .unwrap();
    assert_eq!(
        providers
            .ollama
            .as_ref()
            .expect("ollama endpoint")
            .base_url
            .as_str(),
        "http://10.0.0.5:11434/v1"
    );

    providers
        .set_endpoint("composer", "http://127.0.0.1:8787/v1")
        .unwrap();
    assert_eq!(
        providers.custom["composer"].base_url.as_str(),
        "http://127.0.0.1:8787/v1"
    );

    let unsupported = providers
        .set_endpoint("openai", "https://api.openai.com/v1")
        .unwrap_err();
    assert!(
        format!("{unsupported:#}").contains("conflicts with a built-in provider"),
        "{unsupported:#}"
    );

    let invalid = providers
        .set_endpoint("ollama", "file:///tmp/ollama")
        .unwrap_err();
    assert!(
        format!("{invalid:#}").contains("providers.ollama.base_url"),
        "{invalid:#}"
    );
}

// Covers: Token Plan uses the built-in default API base with no config override
// Owner: provider config
#[test]
fn qwen_token_plan_resolves_default_endpoint_without_config() {
    let config = Config::default();

    assert_eq!(
        config
            .resolved_provider_endpoint("qwen-token-plan")
            .unwrap()
            .as_str(),
        rho_providers::model::registry::QWEN_TOKEN_PLAN_API_BASE
    );
}

// Covers: Meta Model API uses the built-in default API base with no config override
// Owner: provider config
#[test]
fn meta_resolves_default_endpoint_without_config() {
    let config = Config::default();

    assert_eq!(
        config.resolved_provider_endpoint("meta").unwrap().as_str(),
        rho_providers::model::registry::META_API_BASE
    );
}

// Covers: user-defined OpenAI-compatible hosts keep their configured base URL
// Owner: provider config
#[test]
fn custom_openai_compatible_resolves_configured_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"composer\"\nmodel = \"composer-2.5\"\n[providers.custom.composer]\nbase_url = \"http://127.0.0.1:8787/v1\"\n",
    )
    .unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(config.provider, "composer");
    assert_eq!(config.auth, "none");
    assert_eq!(
        config
            .resolved_provider_endpoint("composer")
            .unwrap()
            .as_str(),
        "http://127.0.0.1:8787/v1"
    );
}

// Covers: a stored custom API-key auth profile must survive config load
// Owner: provider config
#[test]
fn custom_openai_compatible_keeps_configured_api_key_auth() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"composer\"\nmodel = \"composer-2.5\"\nauth = \"composer-api-key\"\n[providers.custom.composer]\nbase_url = \"http://127.0.0.1:8787/v1\"\n",
    )
    .unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(config.provider, "composer");
    assert_eq!(config.auth, "composer-api-key");
}

// Covers: custom hosts can borrow a models.dev slug for catalog metadata
// Owner: provider config
#[test]
fn custom_openai_compatible_loads_and_persists_catalog_slug() {
    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"cliproxyapi\"\nmodel = \"gpt-5.6-sol\"\n[providers.custom.cliproxyapi]\nbase_url = \"http://127.0.0.1:8317/v1\"\ncatalog = \"llmgateway\"\n",
    )
    .unwrap();

    let config = Config::load_with_store(
        path.clone(),
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(
        config.providers.custom["cliproxyapi"].catalog.as_deref(),
        Some("llmgateway")
    );

    config.write_settings(path.clone()).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(
        saved.contains("catalog = \"llmgateway\""),
        "saved config must keep catalog: {saved}"
    );
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
}

// Covers: catalog is a models.dev slug, not a provider/model rematch
// Owner: provider config
#[test]
fn custom_catalog_rejects_model_qualified_and_ollama_values() {
    let dir = tempfile::tempdir().unwrap();
    let slash = dir.path().join("slash.toml");
    std::fs::write(
        &slash,
        "[providers.custom.cliproxyapi]\nbase_url = \"http://127.0.0.1:8317/v1\"\ncatalog = \"anthropic/claude-sonnet-4-5\"\n",
    )
    .unwrap();
    let slash_error = Config::load_with_store(
        slash,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap_err();
    assert!(
        format!("{slash_error:#}").contains("providers.custom.cliproxyapi.catalog"),
        "{slash_error:#}"
    );

    let comma = dir.path().join("comma.toml");
    std::fs::write(
        &comma,
        "[providers.custom.cliproxyapi]\nbase_url = \"http://127.0.0.1:8317/v1\"\ncatalog = \"a,b\"\n",
    )
    .unwrap();
    let comma_error = Config::load_with_store(
        comma,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap_err();
    assert!(
        format!("{comma_error:#}").contains("must not contain ','"),
        "{comma_error:#}"
    );

    let ollama = dir.path().join("ollama.toml");
    std::fs::write(
        &ollama,
        "[providers.ollama]\nbase_url = \"http://127.0.0.1:11434/v1\"\ncatalog = \"llmgateway\"\n",
    )
    .unwrap();
    let ollama_error = Config::load_with_store(
        ollama,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap_err();
    assert!(
        format!("{ollama_error:#}").contains("providers.ollama does not accept catalog"),
        "{ollama_error:#}"
    );
}

// Covers: rewriting a custom host URL must not drop a configured catalog
// Owner: provider config
#[test]
fn set_endpoint_preserves_custom_catalog() {
    let mut providers = ProviderConfigs::default();
    providers
        .set_endpoint("cliproxyapi", "http://127.0.0.1:8317/v1")
        .unwrap();
    providers
        .set_catalog("cliproxyapi", Some("llmgateway".into()))
        .unwrap();
    providers
        .set_endpoint("cliproxyapi", "http://127.0.0.1:8318/v1")
        .unwrap();
    assert_eq!(
        providers.custom["cliproxyapi"].catalog.as_deref(),
        Some("llmgateway")
    );
}
