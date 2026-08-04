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

// Covers: Token Plan endpoint from config must drive runtime resolution
// Owner: provider config
#[test]
fn config_loads_qwen_token_plan_base_url() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[providers.qwen-token-plan]\nbase_url = \"https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1\"\n",
    )
    .unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(
        config.providers.qwen_token_plan.base_url.as_str(),
        "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(
        config
            .resolved_provider_endpoint("qwen-token-plan")
            .unwrap()
            .as_str(),
        "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    );
}

// Covers: invalid Token Plan base_url must fail config load loudly
// Owner: provider config
#[test]
fn qwen_token_plan_base_url_rejects_invalid_or_unsupported_urls() {
    for base_url in [
        "not a URL",
        "file:///tmp/qwen",
        "http://user:secret@localhost/compatible-mode/v1",
        "https://token-plan.example/compatible-mode/v1?token=secret",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!("[providers.qwen-token-plan]\nbase_url = {base_url:?}\n"),
        )
        .unwrap();

        let error = Config::load_with_store(
            path,
            &rho_providers::credentials::MemoryCredentialStore::default(),
        )
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("providers.qwen-token-plan.base_url"),
            "{error:#}"
        );
    }
}

// Covers: login and config load must share one validated endpoint write path
// Owner: provider config
#[test]
fn set_endpoint_updates_supported_providers_and_rejects_others() {
    let mut providers = ProviderConfigs::default();

    providers
        .set_endpoint(
            "qwen-token-plan",
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        )
        .unwrap();
    assert_eq!(
        providers.qwen_token_plan.base_url.as_str(),
        "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    );

    providers
        .set_endpoint("ollama", "http://10.0.0.5:11434/v1")
        .unwrap();
    assert_eq!(
        providers.ollama.base_url.as_str(),
        "http://10.0.0.5:11434/v1"
    );

    assert!(ProviderConfigs::stores_endpoint("qwen-token-plan"));
    assert!(!ProviderConfigs::stores_endpoint("openai"));

    let unsupported = providers
        .set_endpoint("openai", "https://api.openai.com/v1")
        .unwrap_err();
    assert!(
        format!("{unsupported:#}").contains("has no configurable base URL"),
        "{unsupported:#}"
    );

    let invalid = providers
        .set_endpoint("qwen-token-plan", "file:///tmp/qwen")
        .unwrap_err();
    assert!(
        format!("{invalid:#}").contains("providers.qwen-token-plan.base_url"),
        "{invalid:#}"
    );
}
