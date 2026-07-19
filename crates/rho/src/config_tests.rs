use super::{Config, LegacyWebSearchCredentials};
use crate::permission::PermissionMode;

#[test]
fn permission_mode_defaults_to_auto_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "provider = \"openai\"\n").unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(config.permission_mode, PermissionMode::Auto);
}

#[test]
fn permission_mode_round_trips_known_values() {
    for mode in [
        PermissionMode::Auto,
        PermissionMode::Plan,
        PermissionMode::Supervised,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config {
            permission_mode: mode,
            ..Config::default()
        };
        let store = rho_providers::credentials::MemoryCredentialStore::default();

        config.save_with_store(path.clone(), &store).unwrap();
        let loaded = Config::load_with_store(path, &store).unwrap();

        assert_eq!(loaded.permission_mode, mode);
    }
}

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
        error
            .to_string()
            .contains("unknown permission mode \"unrestricted\""),
        "{error:#}"
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
fn loads_grouped_config_and_custom_keybinding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
provider = "anthropic"
model = "claude-sonnet-4-5"
reasoning = "high"

[display]
max_tool_output_lines = 24

[keybindings]
jump_to_bottom = "alt+g"
"#,
    )
    .unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert_eq!(config.provider, "anthropic");
    assert_eq!(config.model, "claude-sonnet-4-5");
    assert_eq!(
        config.reasoning,
        rho_providers::reasoning::ReasoningLevel::High
    );
    assert_eq!(config.max_tool_output_lines, 24);
    assert_eq!(config.keybindings.jump_to_bottom.to_string(), "alt+g");
    assert_eq!(config.keybindings.reset_conversation.to_string(), "ctrl+r");
}

#[test]
fn save_organizes_config_into_sections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    Config::default().save(Some(path.clone())).unwrap();

    let saved = std::fs::read_to_string(path).unwrap();
    for section in [
        "[model]",
        "[display]",
        "[output]",
        "[compaction]",
        "[title]",
        "[web_search]",
        "[behavior]",
        "[keybindings]",
    ] {
        assert!(saved.contains(section), "missing {section} in {saved}");
    }
    assert!(!saved.contains("title_provider"), "{saved}");
}

#[test]
fn default_shows_reasoning_output() {
    assert!(Config::default().show_reasoning_output);
}

#[test]
fn loads_reasoning_output_visibility() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "show_reasoning_output = false\n").unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert!(!config.show_reasoning_output);
}

#[test]
fn loads_check_for_updates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "check_for_updates = false\n").unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert!(!config.check_for_updates);
}

#[test]
fn subagents_are_enabled_by_default() {
    assert!(Config::default().enable_subagents);
}

#[test]
fn loads_grouped_subagent_setting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[behavior]\nenable_subagents = false\n").unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert!(!config.enable_subagents);
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
fn loads_rtk_toggle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "rtk = false\n").unwrap();

    let config = Config::load(Some(path)).unwrap();

    assert!(!config.rtk);
}

#[test]
fn loads_and_saves_favorite_models() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
favorite_models = [
  " openai/gpt-5.5 ",
  "missing-separator",
  "openai/gpt-5.5",
  "anthropic/claude-sonnet-4-5",
]
"#,
    )
    .unwrap();

    let config = Config::load(Some(path.clone())).unwrap();

    assert_eq!(
        config.favorite_models,
        vec!["openai/gpt-5.5", "anthropic/claude-sonnet-4-5"]
    );

    config.save(Some(path.clone())).unwrap();
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.contains("favorite_models"), "{saved}");
    assert!(saved.contains("openai/gpt-5.5"), "{saved}");
    assert!(!saved.contains("missing-separator"), "{saved}");
}

#[test]
fn unsupported_web_search_config_providers_fall_back_to_auto() {
    for provider in ["parallel", "tavily", "perplexity", "gemini", "unknown"] {
        assert_eq!(
            super::SearchProvider::from_config_value(provider),
            super::SearchProvider::Auto
        );
    }
}

#[test]
fn supported_web_search_config_provider_is_preserved() {
    assert_eq!(
        super::SearchProvider::from_config_value(" brave "),
        super::SearchProvider::Brave
    );
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

#[test]
fn saved_config_omits_migrated_web_search_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let config = Config::default();

    super::write_config(&path, &config).unwrap();

    let saved = std::fs::read_to_string(path).unwrap();
    assert!(!saved.contains("web_search_openai_api_key"), "{saved}");
    assert!(!saved.contains("web_search_exa_api_key"), "{saved}");
    assert!(!saved.contains("web_search_brave_api_key"), "{saved}");
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
    alias_config(&path, "deep");

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
    alias_config(&path, "fast");

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
fn model_alias_wins_over_identically_named_model_id() {
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

    assert_eq!(config.provider, "anthropic");
    assert_eq!(config.model, "claude-opus-4-8");
    assert_eq!(config.current_model_alias(), Some("gpt-5.5"));
}

#[test]
fn model_alias_targeting_unknown_provider_is_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
model = "deep"

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
        error
            .to_string()
            .contains("model alias 'deep' targets unknown provider 'nonexistent'"),
        "{error:#}"
    );
}

#[test]
fn model_alias_round_trips_through_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    alias_config(&path, "deep");
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let config = Config::load_with_store(path.clone(), &store).unwrap();
    config.save_with_store(path.clone(), &store).unwrap();

    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("model = \"deep\""), "{saved}");
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
    alias_config(&path, "deep");
    let store = rho_providers::credentials::MemoryCredentialStore::default();

    let mut config = Config::load_with_store(path.clone(), &store).unwrap();
    config.model = "claude-haiku-4-5".into();
    assert_eq!(config.current_model_alias(), None);
    config.save_with_store(path.clone(), &store).unwrap();

    let reloaded = Config::load_with_store(path, &store).unwrap();
    assert_eq!(reloaded.model, "claude-haiku-4-5");
    assert_eq!(reloaded.current_model_alias(), None);
}
