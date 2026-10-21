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
        .set_endpoint("ollama", rho_providers::provider::OLLAMA_API_BASE)
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
            rho_providers::provider::OLLAMA_API_BASE
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

// Covers: built-in hosts resolve their default API base with no config override
// Owner: provider config
#[test]
fn built_in_providers_resolve_default_endpoint_without_config() {
    let config = Config::default();

    for (provider, expected) in [
        (
            "qwen-token-plan",
            rho_providers::provider::QWEN_TOKEN_PLAN_API_BASE,
        ),
        ("meta", rho_providers::provider::META_API_BASE),
        ("minimax", rho_providers::provider::MINIMAX_API_BASE),
    ] {
        assert_eq!(
            config
                .resolved_provider_endpoint(provider)
                .unwrap()
                .as_str(),
            expected,
            "{provider}"
        );
    }
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

// Covers: a stored custom key must promote `auth = "none"` so restart sends it
// Owner: provider config
#[test]
fn custom_openai_compatible_promotes_stored_key_over_keyless() {
    use rho_providers::credentials::{CredentialStore, MemoryCredentialStore};

    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"composer\"\nmodel = \"composer-2.5\"\nauth = \"none\"\n[providers.custom.composer]\nbase_url = \"http://127.0.0.1:8787/v1\"\n",
    )
    .unwrap();

    let store = MemoryCredentialStore::default();
    store
        .set_secret("provider:composer:api-key", "composer-secret")
        .unwrap();
    let mut config = Config::load_with_store(path, &store).unwrap();
    assert_eq!(config.auth, "none");
    assert!(config.promote_stored_custom_auth(&store));
    assert_eq!(config.auth, "composer-api-key");
}

// Covers: startup must not write keyed custom auth back to `none`
// Owner: provider config
#[test]
fn custom_openai_compatible_keeps_keyed_auth_when_store_is_empty() {
    use rho_providers::credentials::MemoryCredentialStore;

    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"composer\"\nmodel = \"composer-2.5\"\nauth = \"composer-api-key\"\n[providers.custom.composer]\nbase_url = \"http://127.0.0.1:8787/v1\"\n",
    )
    .unwrap();

    let store = MemoryCredentialStore::default();
    let mut config = Config::load_with_store(path, &store).unwrap();
    assert_eq!(config.auth, "composer-api-key");
    assert!(!config.promote_stored_custom_auth(&store));
    assert_eq!(config.auth, "composer-api-key");
}

// Covers: endpoint updates change only their declared fields, so current and
// future provider options cannot be erased by an incomplete copy list.
// Owner: provider config
#[test]
fn endpoint_updates_preserve_unrelated_custom_options() {
    let mut providers = ProviderConfigs::default();
    let original = ProviderEndpointConfig {
        base_url: Url::parse("http://127.0.0.1:8317/v1").unwrap(),
        catalog: Some("llmgateway".into()),
        catalog_lookup: rho_providers::provider::CatalogLookupMode::Slug,
        api: rho_providers::provider::OpenAiCompatibleApi::Responses,
        edit_tool: Some(rho_tools::EditFormat::ApplyPatch),
        prompt_cache_ttl: Some(std::time::Duration::from_secs(3600)),
        request_compression: Some(rho_providers::providers::RequestCompression::Zstd),
    };
    providers
        .custom
        .insert("cliproxyapi".into(), original.clone());

    providers
        .set_endpoint("cliproxyapi", "http://127.0.0.1:8318/v1")
        .unwrap();
    let mut expected = original;
    expected.base_url = Url::parse("http://127.0.0.1:8318/v1").unwrap();
    assert_eq!(providers.custom["cliproxyapi"], expected);

    providers
        .set_custom_endpoint(
            "cliproxyapi",
            "http://127.0.0.1:8319/v1",
            rho_providers::provider::OpenAiCompatibleApi::ChatCompletions,
        )
        .unwrap();
    expected.base_url = Url::parse("http://127.0.0.1:8319/v1").unwrap();
    expected.api = rho_providers::provider::OpenAiCompatibleApi::ChatCompletions;
    assert_eq!(providers.custom["cliproxyapi"], expected);
}

// Covers: a custom provider can choose Auto's edit format without overriding a
// globally pinned edit preference, and the choice survives config persistence.
// Owner: provider config
#[test]
fn custom_provider_edit_tool_configures_auto_resolution() {
    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"vllm\"\nmodel = \"qwen2.5-coder\"\n[providers.custom.vllm]\nbase_url = \"http://127.0.0.1:8000/v1\"\nedit_tool = \"apply_patch\"\n[behavior]\nedit_tool = \"auto\"\n",
    )
    .unwrap();

    let mut config = Config::load_with_store(
        path.clone(),
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(
        config.resolved_edit_tool(),
        rho_tools::EditFormat::ApplyPatch
    );
    config.edit_tool = crate::config::EditTool::Pinned(rho_tools::EditFormat::StrReplace);
    assert_eq!(
        config.resolved_edit_tool(),
        rho_tools::EditFormat::StrReplace
    );

    config.write_settings(path.clone()).unwrap();
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(
        saved.contains("edit_tool = \"apply_patch\""),
        "saved config must keep the custom provider edit tool: {saved}"
    );
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
}

// Covers: every custom endpoint option loads, interns into provider
// construction, and survives a settings write and reload, while an omitted
// api stays Chat Completions.
// Owner: provider config
#[test]
fn custom_endpoint_options_load_intern_and_persist() {
    use rho_providers::provider::{
        interned_custom_provider, CatalogLookupMode, OpenAiCompatibleApi,
    };

    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"litellm\"\nmodel = \"gpt-5.6\"\n[providers.custom.composer]\nbase_url = \"http://127.0.0.1:8787/v1\"\ncatalog_mode = \"model-id\"\n[providers.custom.litellm]\nbase_url = \"http://127.0.0.1:4000/v1\"\napi = \"responses\"\ncatalog = \"llmgateway\"\nprompt_cache_ttl = \"1h\"\nrequest_compression = \"zstd\"\n",
    )
    .unwrap();
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let config = Config::load_with_store(path.clone(), &store).unwrap();

    let litellm = &config.providers.custom["litellm"];
    assert_eq!(
        (
            litellm.api,
            litellm.catalog.as_deref(),
            litellm.prompt_cache_ttl,
            litellm.request_compression,
        ),
        (
            OpenAiCompatibleApi::Responses,
            Some("llmgateway"),
            Some(std::time::Duration::from_secs(3600)),
            Some(rho_providers::providers::RequestCompression::Zstd),
        )
    );
    let composer = &config.providers.custom["composer"];
    assert_eq!(
        (composer.api, composer.catalog_lookup),
        (
            OpenAiCompatibleApi::ChatCompletions,
            CatalogLookupMode::ModelId
        )
    );
    let litellm = interned_custom_provider("litellm").unwrap();
    let composer = interned_custom_provider("composer").unwrap();
    assert_eq!(
        (
            litellm.openai_compatible_api(),
            composer.openai_compatible_api(),
            composer.catalog_lookup(),
        ),
        (
            OpenAiCompatibleApi::Responses,
            OpenAiCompatibleApi::ChatCompletions,
            CatalogLookupMode::ModelId,
        )
    );

    config.write_settings(path.clone()).unwrap();
    let reloaded = Config::load_with_store(path, &store).unwrap();
    assert_eq!(reloaded.providers, config.providers);
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
}

// Covers: invalid endpoint options fail config load naming the offending
// field, and Ollama refuses the custom-only options.
// Owner: provider config
#[test]
fn invalid_endpoint_options_name_the_offending_field() {
    let custom = "[providers.custom.cliproxy]\nbase_url = \"http://127.0.0.1:8317/v1\"\n";
    let ollama = "[providers.ollama]\nbase_url = \"http://127.0.0.1:11434/v1\"\n";
    let cases = [
        (
            "model-qualified catalog",
            format!("{custom}catalog = \"anthropic/claude-sonnet-4-5\"\n"),
            "providers.custom.cliproxy.catalog",
        ),
        (
            "comma catalog",
            format!("{custom}catalog = \"a,b\"\n"),
            "must not contain ','",
        ),
        (
            "unknown catalog_mode",
            format!("{custom}catalog_mode = \"owner-model\"\n"),
            "providers.custom.cliproxy.catalog_mode",
        ),
        (
            "catalog_mode with catalog",
            format!("{custom}catalog = \"llmgateway\"\ncatalog_mode = \"model-id\"\n"),
            "cannot be combined with catalog",
        ),
        (
            "unknown api",
            format!("{custom}api = \"websocket\"\n"),
            "providers.custom.cliproxy.api",
        ),
        (
            "zero prompt_cache_ttl",
            format!("{custom}prompt_cache_ttl = \"0s\"\n"),
            "providers.custom.cliproxy.prompt_cache_ttl",
        ),
        (
            "unknown request_compression",
            format!("{custom}request_compression = \"br\"\n"),
            "providers.custom.cliproxy.request_compression",
        ),
        (
            "ollama catalog",
            format!("{ollama}catalog = \"llmgateway\"\n"),
            "providers.ollama does not accept catalog",
        ),
        (
            "ollama catalog_mode",
            format!("{ollama}catalog_mode = \"model-id\"\n"),
            "providers.ollama does not accept catalog_mode",
        ),
        (
            "ollama api",
            format!("{ollama}api = \"responses\"\n"),
            "providers.ollama does not accept api",
        ),
        (
            "ollama request_compression",
            format!("{ollama}request_compression = \"zstd\"\n"),
            "providers.ollama does not accept request_compression",
        ),
    ];
    let dir = tempfile::tempdir().unwrap();
    let store = rho_providers::credentials::MemoryCredentialStore::default();
    for (case, table, field) in cases {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, table).unwrap();

        let error = Config::load_with_store(path, &store).unwrap_err();

        assert!(format!("{error:#}").contains(field), "{case}: {error:#}");
    }
}
