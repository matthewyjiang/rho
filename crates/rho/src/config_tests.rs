use super::{Config, EffectiveModelSource, LegacyWebSearchCredentials};

#[test]
fn unknown_permission_mode_is_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "permission_mode = \"unrestricted\"\n").unwrap();

    let error = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap_err();

    assert!(
        format!("{error:#}").contains("unknown permission mode \"unrestricted\""),
        "{error:#}"
    );
}

// Covers: invalid config errors must name the config file path
// Owner: config load
#[test]
fn invalid_config_errors_include_file_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken-config.toml");
    std::fs::write(&path, "permission_mode = [\n").unwrap();

    let error = Config::load(Some(path.clone())).unwrap_err();
    let message = format!("{error:#}");

    assert!(
        message.contains(&path.display().to_string()),
        "error must name the config file path: {message}"
    );
    assert!(
        message.contains("failed to parse config file"),
        "error must mark the parse step: {message}"
    );
}

#[test]
fn config_debug_redacts_legacy_credentials() {
    let config = Config {
        legacy_web_search_credentials: LegacyWebSearchCredentials {
            openai: Some("openai-search-secret".into()),
            exa: Some("exa-search-secret".into()),
            brave: Some("brave-search-secret".into()),
        },
        ..Config::default()
    };

    let debug = format!("{config:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("openai-search-secret"));
    assert!(!debug.contains("exa-search-secret"));
    assert!(!debug.contains("brave-search-secret"));
}

#[test]
fn migrates_the_legacy_ctrl_g_shortcut_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[keybindings]
jump_to_bottom = "ctrl+g"
"#,
    )
    .unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert_eq!(config.keybindings.open_editor.to_string(), "ctrl+g");
    assert_eq!(config.keybindings.jump_to_bottom.to_string(), "ctrl+end");
}

#[test]
fn preserves_explicit_distinct_editor_and_jump_shortcuts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[keybindings]
open_editor = "alt+e"
jump_to_bottom = "ctrl+g"
"#,
    )
    .unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert_eq!(config.keybindings.open_editor.to_string(), "alt+e");
    assert_eq!(config.keybindings.jump_to_bottom.to_string(), "ctrl+g");
}

#[test]
fn rejects_explicit_editor_and_jump_shortcut_collisions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[keybindings]
open_editor = "ctrl+g"
jump_to_bottom = "ctrl+g"
"#,
    )
    .unwrap();

    let error = Config::load(Some(path)).unwrap_err();

    assert!(
        format!("{error:#}").contains("open_editor and jump_to_bottom must use different keys"),
        "{error:#}"
    );
}

#[test]
fn loads_and_normalizes_compaction_percentages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "auto_compact = true\ncompact_threshold_percent = 80\ncompact_target_percent = 95\n",
    )
    .unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert!(config.auto_compact);
    assert_eq!(config.compact_threshold_percent, 80);
    assert_eq!(config.compact_target_percent, 79);
}

#[test]
fn unsupported_web_search_config_providers_fall_back_to_auto() {
    for provider in ["parallel", "tavily", "perplexity", "gemini", "unknown"] {
        assert_eq!(
            super::SearchProvider::parse_config_value(provider),
            (super::SearchProvider::Auto, true)
        );
    }
}

#[test]
fn grouped_web_search_loads_hosted_flag() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[web_search]
hosted = false
provider = "brave"
"#,
    )
    .unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert!(!config.web_search_hosted);
    assert_eq!(config.web_search_provider, super::SearchProvider::Brave);
}

#[test]
fn grouped_web_search_defaults_hosted_on_when_omitted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[web_search]
provider = "exa"
"#,
    )
    .unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert!(config.web_search_hosted);
    assert_eq!(config.web_search_provider, super::SearchProvider::Exa);
}

// Covers: omitted [xai] stays on; explicit image_generation = false disables it.
// Owner: config load
#[test]
fn grouped_xai_image_generation_defaults_on_and_can_disable() {
    let dir = tempfile::tempdir().unwrap();
    let default_path = dir.path().join("default.toml");
    std::fs::write(&default_path, "").unwrap();
    assert!(
        Config::load(Some(default_path))
            .unwrap()
            .xai_image_generation
    );

    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[xai]
image_generation = false
"#,
    )
    .unwrap();

    assert!(!Config::load(Some(path)).unwrap().xai_image_generation);
}

#[test]
fn grouped_web_search_preserves_omitted_legacy_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
web_search_openai_api_key = "legacy-openai"
web_search_exa_api_key = "legacy-exa"

[web_search]
provider = "brave"
brave_api_key = "grouped-brave"
"#,
    )
    .unwrap();
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let config = Config::load_with_store(path, &store).unwrap();

    assert_eq!(config.web_search_provider, super::SearchProvider::Brave);
    for (credential, expected) in [
        (
            rho_providers::credentials::WebSearchCredential::OpenAi,
            "legacy-openai",
        ),
        (
            rho_providers::credentials::WebSearchCredential::Exa,
            "legacy-exa",
        ),
        (
            rho_providers::credentials::WebSearchCredential::Brave,
            "grouped-brave",
        ),
    ] {
        assert_eq!(
            rho_providers::credentials::load_web_search_api_key(&store, credential)
                .unwrap()
                .as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn save_preserves_legacy_web_search_keys_when_credentials_are_unavailable() {
    struct UnavailableCredentialStore;

    impl rho_providers::credentials::CredentialStore for UnavailableCredentialStore {
        fn get_secret(
            &self,
            _account: &str,
        ) -> rho_providers::credentials::CredentialResult<Option<String>> {
            Err(
                rho_providers::credentials::CredentialError::StoreUnavailable(
                    "test store unavailable".into(),
                ),
            )
        }

        fn set_secret(
            &self,
            _account: &str,
            _secret: &str,
        ) -> rho_providers::credentials::CredentialResult<()> {
            Err(
                rho_providers::credentials::CredentialError::StoreUnavailable(
                    "test store unavailable".into(),
                ),
            )
        }

        fn delete_secret(
            &self,
            _account: &str,
        ) -> rho_providers::credentials::CredentialResult<bool> {
            Err(
                rho_providers::credentials::CredentialError::StoreUnavailable(
                    "test store unavailable".into(),
                ),
            )
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let config = Config {
        rtk: false,
        legacy_web_search_credentials: super::LegacyWebSearchCredentials {
            openai: Some("sk-test".into()),
            exa: None,
            brave: None,
        },
        ..Config::default()
    };

    config
        .save_with_store(path.clone(), &UnavailableCredentialStore)
        .unwrap();

    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.contains("openai_api_key = \"sk-test\""), "{saved}");
    assert!(saved.contains("rtk = false"), "{saved}");
}

#[cfg(unix)]
#[test]
fn migrated_credential_cleanup_atomically_replaces_read_only_config() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "web_search_openai_api_key = \"sk-test\"\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let config = Config::load_with_store(path.clone(), &store).unwrap();

    assert_eq!(
        rho_providers::credentials::load_web_search_api_key(
            &store,
            rho_providers::credentials::WebSearchCredential::OpenAi
        )
        .unwrap()
        .as_deref(),
        Some("sk-test")
    );
    assert_eq!(
        config.legacy_web_search_api_key(rho_providers::credentials::WebSearchCredential::OpenAi),
        None
    );
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(!saved.contains("web_search_openai_api_key"), "{saved}");

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn migrates_legacy_web_search_keys_to_credentials() {
    let store = rho_providers::credentials::MemoryCredentialStore::default();
    let mut config = Config {
        legacy_web_search_credentials: super::LegacyWebSearchCredentials {
            openai: Some("sk-test".into()),
            exa: Some("exa-test".into()),
            brave: Some("BSA-test".into()),
        },
        ..Config::default()
    };

    assert!(config
        .migrate_legacy_web_search_credentials(&store)
        .unwrap());
    assert_eq!(
        rho_providers::credentials::load_web_search_api_key(
            &store,
            rho_providers::credentials::WebSearchCredential::OpenAi
        )
        .unwrap()
        .as_deref(),
        Some("sk-test")
    );
    assert_eq!(
        config.legacy_web_search_api_key(rho_providers::credentials::WebSearchCredential::OpenAi),
        None
    );
}

fn alias_config(path: &std::path::Path, model: &str) {
    std::fs::write(
        path,
        format!(
            r#"
[model]
provider = "openai"
model = "{model}"

[model.aliases]
deep = "anthropic/claude-opus-4-8"
fast = "gpt-5.5-mini"
"#
        ),
    )
    .unwrap();
}

#[test]
fn model_alias_resolves_session_model_at_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "@deep");

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(config.provider, "anthropic");
    assert_eq!(config.model, "claude-opus-4-8");
    assert_eq!(config.current_model_alias(), Some("deep"));
}

#[test]
fn bare_model_alias_keeps_configured_provider_and_auth() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "@fast");

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(config.provider, "openai");
    assert_eq!(config.model, "gpt-5.5-mini");
    assert_eq!(config.auth, Config::default().auth);
    assert_eq!(config.current_model_alias(), Some("fast"));
}

#[test]
fn concrete_model_id_wins_over_identically_named_alias() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
provider = "openai"
model = "gpt-5.5"

[model.aliases]
"gpt-5.5" = "anthropic/claude-opus-4-8"
"#,
    )
    .unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(config.provider, "openai");
    assert_eq!(config.model, "gpt-5.5");
    assert_eq!(config.current_model_alias(), None);
}

#[test]
fn model_alias_targeting_unknown_provider_is_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
model = "@deep"

[model.aliases]
deep = "nonexistent/model-x"
"#,
    )
    .unwrap();

    let error = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap_err();

    assert!(
        format!("{error:#}").contains("model alias 'deep' targets unknown provider 'nonexistent'"),
        "{error:#}"
    );
}

#[test]
fn undefined_session_model_alias_names_reference_site() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[model]\nmodel = \"@missing\"\n").unwrap();

    let error = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap_err();

    assert!(
        format!("{error:#}").contains(
            "session model: model alias '@missing' is not defined; define it in [model.aliases] or use a concrete model reference"
        ),
        "{error:#}"
    );
}

#[test]
fn undefined_title_model_alias_names_reference_site() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[model]\nmodel = \"gpt-5.5\"\n\n[title]\nmodel = \"@missing\"\n",
    )
    .unwrap();

    let error = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap_err();

    assert!(
        format!("{error:#}").contains(
            "internal agent 'session-title' model: model alias '@missing' is not defined; define it in [model.aliases] or use a concrete model reference"
        ),
        "{error:#}"
    );
}

#[test]
fn unused_model_alias_targeting_unknown_provider_is_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
model = "gpt-5.5"

[model.aliases]
unused = "nonexistent/model-x"
"#,
    )
    .unwrap();

    let error = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap_err();

    assert!(
        format!("{error:#}")
            .contains("model alias 'unused' targets unknown provider 'nonexistent'"),
        "{error:#}"
    );
}

#[test]
fn title_model_alias_resolves_provider_model_and_auth_at_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
provider = "openai"
model = "gpt-5.5"

[model.aliases]
titler = "anthropic/claude-haiku-4-5"

[title]
provider = "anthropic"
model = "@titler"
"#,
    )
    .unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    let selection = config.internal_agent_model("session-title").unwrap();
    assert_eq!(selection.expect_rho().provider, "anthropic");
    assert_eq!(selection.expect_rho().model, "claude-haiku-4-5");
    assert_eq!(selection.expect_rho().auth, "anthropic-api-key");
    assert_eq!(
        config.current_internal_agent_model_alias("session-title"),
        Some("titler")
    );
}

#[test]
fn stale_title_model_alias_saves_the_concrete_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "gpt-5.5");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n[title]\nmodel = \"@deep\"\n");
    std::fs::write(&path, text).unwrap();
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let mut config = Config::load_with_store(path.clone(), &store).unwrap();
    config
        .internal_agents
        .get_mut("session-title")
        .unwrap()
        .expect_rho_mut()
        .model = "claude-sonnet-4-5".into();
    assert_eq!(
        config.current_internal_agent_model_alias("session-title"),
        None
    );
    config.save_with_store(path.clone(), &store).unwrap();

    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("model = \"claude-sonnet-4-5\""), "{saved}");
    let reloaded = Config::load_with_store(path, &store).unwrap();
    assert_eq!(
        reloaded
            .internal_agent_model("session-title")
            .unwrap()
            .expect_rho()
            .model,
        "claude-sonnet-4-5"
    );
    assert_eq!(
        reloaded.current_internal_agent_model_alias("session-title"),
        None
    );
}

#[test]
fn effective_internal_agent_models_are_independent() {
    let mut config = Config::default();
    config.set_internal_agent_model(
        "session-title",
        "anthropic".into(),
        "claude-haiku-4-5".into(),
        "anthropic-api-key".into(),
    );

    let title = config.effective_internal_agent_model("session-title");
    let judge = config.effective_internal_agent_model("goal-judge");

    assert_eq!(title.provider, "anthropic");
    assert_eq!(title.model, "claude-haiku-4-5");
    assert_eq!(title.source, EffectiveModelSource::Override);
    assert_eq!(judge.provider, config.provider);
    assert_eq!(judge.model, config.model);
    assert_eq!(judge.auth, config.auth);
    assert_eq!(judge.source, EffectiveModelSource::Conversation);
}

#[test]
fn model_alias_round_trips_through_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "@deep");
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let config = Config::load_with_store(path.clone(), &store).unwrap();
    config.save_with_store(path.clone(), &store).unwrap();

    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("model = \"@deep\""), "{saved}");
    assert!(saved.contains("[model.aliases]"), "{saved}");
    let reloaded = Config::load_with_store(path, &store).unwrap();
    assert_eq!(reloaded.provider, "anthropic");
    assert_eq!(reloaded.model, "claude-opus-4-8");
    assert_eq!(reloaded.current_model_alias(), Some("deep"));
}

#[test]
fn stale_model_alias_saves_the_concrete_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "@deep");
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let mut config = Config::load_with_store(path.clone(), &store).unwrap();
    config.model = "claude-haiku-4-5".into();
    assert_eq!(config.current_model_alias(), None);
    config.save_with_store(path.clone(), &store).unwrap();

    let reloaded = Config::load_with_store(path, &store).unwrap();
    assert_eq!(reloaded.model, "claude-haiku-4-5");
    assert_eq!(reloaded.current_model_alias(), None);
}

#[test]
fn flat_title_settings_migrate_to_internal_agent_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "title_provider = \"anthropic\"\ntitle_model = \"claude-haiku-4-5\"\ntitle_auth = \"anthropic-api-key\"\n",
    )
    .unwrap();
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let config = Config::load_with_store(path.clone(), &store).unwrap();
    let title = config.internal_agent_model("session-title").unwrap();
    assert_eq!(
        (
            title.expect_rho().provider.as_str(),
            title.expect_rho().model.as_str(),
            title.expect_rho().auth.as_str()
        ),
        ("anthropic", "claude-haiku-4-5", "anthropic-api-key")
    );

    config.save_with_store(path.clone(), &store).unwrap();
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.contains("[internal_agents.session-title]"), "{saved}");
    assert!(!saved.contains("title_provider"), "{saved}");
    assert!(!saved.contains("[title]"), "{saved}");
}

#[test]
fn generic_internal_agent_config_wins_over_legacy_title_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[title]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"

[internal_agents.session-title]
provider = "anthropic"
model = "claude-haiku-4-5"
auth = "anthropic-api-key"
"#,
    )
    .unwrap();
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let config = Config::load_with_store(path, &store).unwrap();
    let title = config.internal_agent_model("session-title").unwrap();
    assert_eq!(
        (
            title.expect_rho().provider.as_str(),
            title.expect_rho().model.as_str(),
            title.expect_rho().auth.as_str()
        ),
        ("anthropic", "claude-haiku-4-5", "anthropic-api-key")
    );
}

#[test]
fn empty_legacy_title_section_remains_conversation_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[model]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nauth = \"api-key\"\n\n[title]\n",
    )
    .unwrap();
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let config = Config::load_with_store(path.clone(), &store).unwrap();
    assert!(config.internal_agent_model("session-title").is_none());
    assert_eq!(
        config
            .effective_internal_agent_model("session-title")
            .source,
        EffectiveModelSource::Conversation
    );

    config.save_with_store(path.clone(), &store).unwrap();
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(!saved.contains("[title]"), "{saved}");
    assert!(
        !saved.contains("[internal_agents.session-title]"),
        "{saved}"
    );
}

#[test]
fn provider_base_url_rejects_invalid_or_unsupported_urls() {
    for (section, field) in [
        ("[providers.ollama]", "providers.ollama.base_url"),
        (
            "[providers.custom.composer]",
            "providers.custom.composer.base_url",
        ),
    ] {
        for base_url in [
            "not a URL",
            "file:///tmp/models",
            "http://user:secret@localhost:11434/v1",
            "http://localhost:11434/v1?token=secret",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, format!("{section}\nbase_url = {base_url:?}\n")).unwrap();

            let error = Config::load_with_store(
                path,
                &rho_providers::credentials::MemoryCredentialStore::default(),
            )
            .unwrap_err();

            assert!(format!("{error:#}").contains(field), "{error:#}");
        }
    }
}

#[test]
fn keyless_provider_inference_sets_no_auth_without_login_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
provider = "openai"
model = "@local"
auth = "api-key"

[model.aliases]
local = "ollama/local-model"

[internal_agents.goal-judge]
provider = "ollama"
model = "judge-model"

[title]
provider = "ollama"
model = "title-model"
"#,
    )
    .unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(config.provider, "ollama");
    assert_eq!(config.auth, "none");
    assert_eq!(
        config
            .internal_agent_model("goal-judge")
            .unwrap()
            .expect_rho()
            .auth,
        "none"
    );
    assert_eq!(
        config
            .internal_agent_model("session-title")
            .unwrap()
            .expect_rho()
            .auth,
        "none"
    );
}

// Covers: advisor mode, model, and reasoning survive a save and reload
// Owner: config persistence
#[test]
fn advisor_mode_and_model_round_trip_through_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let store = rho_providers::credentials::MemoryCredentialStore::default();
    let mut config = Config {
        advisor_mode: true,
        ..Default::default()
    };
    let mut advisor = crate::config::InternalAgentModelConfig::new(
        "anthropic".into(),
        "claude-opus-4-8".into(),
        "anthropic-api-key".into(),
    );
    advisor.reasoning = Some(rho_providers::reasoning::ReasoningLevel::High);
    config.set_internal_agent_model_config(crate::agent::ADVISOR_AGENT_ID, advisor.clone());

    config.save_with_store(path.clone(), &store).unwrap();
    let reloaded = Config::load_with_store(path, &store).unwrap();

    assert!(reloaded.advisor_mode);
    assert_eq!(
        reloaded
            .internal_agent_model(crate::agent::ADVISOR_AGENT_ID)
            .cloned(),
        Some(advisor)
    );
}

// Covers: a delegating advisor must survive save and reload as a delegating
// advisor, and must not persist a Rho provider or auth it does not have.
// Owner: config persistence
#[test]
fn a_claude_cli_advisor_round_trips_through_save() {
    for model in [Some("opus".to_string()), None] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let store = rho_providers::credentials::MemoryCredentialStore::default();
        let mut config = Config {
            advisor_mode: true,
            ..Default::default()
        };
        let mut advisor = crate::config::InternalAgentModelConfig::claude_cli(model.clone());
        advisor.reasoning = Some(rho_providers::reasoning::ReasoningLevel::High);
        config.set_internal_agent_model_config(crate::agent::ADVISOR_AGENT_ID, advisor.clone());

        config.save_with_store(path.clone(), &store).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        let saved_toml: toml::Value = toml::from_str(&saved).unwrap();
        let advisor_table = saved_toml["internal_agents"]["advisor"]
            .as_table()
            .expect("serialized advisor table");
        assert_eq!(
            advisor_table.get("runtime").and_then(toml::Value::as_str),
            Some("claude-cli")
        );
        assert!(!advisor_table.contains_key("provider"), "{saved}");
        assert!(!advisor_table.contains_key("auth"), "{saved}");

        let reloaded = Config::load_with_store(path, &store).unwrap();
        assert_eq!(
            reloaded
                .internal_agent_model(crate::agent::ADVISOR_AGENT_ID)
                .cloned(),
            Some(advisor),
            "model {model:?} did not round trip"
        );
    }
}
