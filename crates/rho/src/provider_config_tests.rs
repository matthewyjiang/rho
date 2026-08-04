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
        (title.provider.as_str(), title.auth.as_str()),
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
    assert_eq!(title.provider, "poolside");
    assert_eq!(title.model, "laguna-m.1");
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
        providers.ollama.base_url.as_str(),
        "http://10.0.0.5:11434/v1"
    );

    let unsupported = providers
        .set_endpoint("openai", "https://api.openai.com/v1")
        .unwrap_err();
    assert!(
        format!("{unsupported:#}").contains("has no configurable base URL"),
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
