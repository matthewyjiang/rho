use clap::Parser;
use std::time::{SystemTime, UNIX_EPOCH};

use {
    crate::cli::{Cli, Command},
    crate::config::Config,
    rho_providers::credentials::{
        save_github_copilot_tokens, GitHubCopilotTokens, MemoryCredentialStore,
    },
    rho_providers::model::{
        provider_models::{
            replace_cached_provider_models_for_tests, set_provider_models_cache_dir_for_tests,
            with_provider_models_cache_dir_for_tests, ProviderModel,
        },
        ReasoningCapabilities, ReasoningLevelSet,
    },
};

use super::{
    apply_overrides, normalize_reasoning, normalize_reasoning_for_cli, refresh_model_cache,
    validate,
};

fn unique_cache_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock should be after Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "rho-cli-config-{name}-{}-{nanos}",
        std::process::id()
    ))
}

fn with_cached_provider_models<T>(provider: &str, models: Vec<&str>, f: impl FnOnce() -> T) -> T {
    let cache_dir = unique_cache_dir(provider);
    let provider_models = models
        .into_iter()
        .map(|model| ProviderModel {
            provider: provider.into(),
            model: model.into(),
            display_name: model.into(),
            context_window: None,
            max_output_tokens: None,
            reasoning_capabilities: ReasoningCapabilities::Unknown,
        })
        .collect::<Vec<_>>();
    let result = with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models_for_tests(provider, &provider_models).unwrap();
        f()
    });
    let _ = std::fs::remove_dir_all(cache_dir);
    result
}

#[test]
fn validate_cli_rejects_resume_with_run_before_prompt_reading() {
    let cli = Cli {
        provider: None,
        model: None,
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: Some(Some("session-id".into())),
        command: Some(Command::Run {
            stdin: true,
            output_file: None,
            output: crate::cli::OutputFormat::Text,
            max_steps: None,
            timeout: None,
            prompt: Vec::new(),
        }),
    };

    let err = validate(&cli).unwrap_err();

    assert!(err.to_string().contains("--resume is only supported"));
}

#[test]
fn validate_cli_rejects_resume_with_update() {
    let cli = Cli {
        provider: None,
        model: None,
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: Some(Some("session-id".into())),
        command: Some(Command::Update),
    };

    let err = validate(&cli).unwrap_err();

    assert!(err.to_string().contains("--resume is only supported"));
}

#[test]
fn clean_poolside_model_override_persists_namespaced_wire_id() {
    with_cached_provider_models("poolside", vec!["poolside/laguna-m.1"], || {
        let mut config = Config::default();
        let cli = Cli::try_parse_from(["rho", "--model", "poolside/laguna-m.1"]).unwrap();

        assert!(apply_overrides(&mut config, &cli).unwrap());
        assert_eq!(config.provider, "poolside");
        assert_eq!(config.model, "poolside/laguna-m.1");
        assert_eq!(config.auth, "poolside-api-key");
    });
}

#[test]
fn cli_model_override_with_provider_selects_matching_auth() {
    let mut cfg = Config::default();
    let cli = Cli {
        provider: None,
        model: Some("openai-codex/gpt-5.4-mini".into()),
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: None,
        command: None,
    };

    let save_config = apply_overrides(&mut cfg, &cli).unwrap();

    assert!(save_config);
    assert_eq!(cfg.provider, "openai-codex");
    assert_eq!(cfg.model, "gpt-5.4-mini");
    assert_eq!(cfg.auth, "codex");
}

#[test]
fn cli_anthropic_model_override_selects_matching_auth() {
    with_cached_provider_models("anthropic", vec!["claude-sonnet-4-5"], || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: None,
            model: Some("anthropic/claude-sonnet-4-5".into()),
            config: None,
            auth: None,
            no_system_prompt: false,
            no_tools: false,
            no_subagents: false,
            agent: None,
            reasoning: None,
            resume: None,
            command: None,
        };

        let save_config = apply_overrides(&mut cfg, &cli).unwrap();

        assert!(save_config);
        assert_eq!(cfg.provider, "anthropic");
        assert_eq!(cfg.model, "claude-sonnet-4-5");
        assert_eq!(cfg.auth, "anthropic-api-key");
    });
}

#[test]
fn cli_anthropic_provider_override_uses_cached_default() {
    with_cached_provider_models("anthropic", vec!["claude-sonnet-4-5"], || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: Some("anthropic".into()),
            model: None,
            config: None,
            auth: None,
            no_system_prompt: false,
            no_tools: false,
            no_subagents: false,
            agent: None,
            reasoning: None,
            resume: None,
            command: None,
        };

        let save_config = apply_overrides(&mut cfg, &cli).unwrap();

        assert!(save_config);
        assert_eq!(cfg.provider, "anthropic");
        assert_eq!(cfg.model, "claude-sonnet-4-5");
        assert_eq!(cfg.auth, "anthropic-api-key");
    });
}

#[test]
fn cli_anthropic_provider_override_without_cache_uses_builtin_default() {
    let mut cfg = Config::default();
    let cli = Cli {
        provider: Some("anthropic".into()),
        model: None,
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: None,
        command: None,
    };

    apply_overrides(&mut cfg, &cli).unwrap();

    assert_eq!(cfg.provider, "anthropic");
    assert_eq!(cfg.model, "claude-sonnet-4-5");
    assert_eq!(cfg.auth, "anthropic-api-key");
}

#[test]
fn cli_github_copilot_provider_override_requires_cached_default() {
    let cache_dir = unique_cache_dir("github-copilot-empty");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: Some("github-copilot".into()),
            model: None,
            config: None,
            auth: None,
            no_system_prompt: false,
            no_tools: false,
            no_subagents: false,
            agent: None,
            reasoning: None,
            resume: None,
            command: None,
        };

        let err = apply_overrides(&mut cfg, &cli).unwrap_err();

        assert!(err.to_string().contains("no cached models"));
    });
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[tokio::test]
async fn cli_github_copilot_provider_override_refreshes_empty_cache() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let models_url = format!("http://{}/models", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 1024];
        let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
            .await
            .unwrap();
        let body = r#"{"data":[{"id":"copilot-api-model"}]}"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, reply.as_bytes())
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::shutdown(&mut stream)
            .await
            .unwrap();
    });
    let cache_dir = unique_cache_dir("github-copilot-refresh");
    let store = MemoryCredentialStore::default();
    save_github_copilot_tokens(
        &store,
        &GitHubCopilotTokens {
            github_access_token: "github".into(),
            github_refresh_token: None,
            github_expires_at_unix: None,
            copilot_token: Some("copilot-test-token".into()),
            copilot_expires_at_unix: Some(i64::MAX),
            copilot_refresh_after_unix: None,
            copilot_token_endpoint: None,
            copilot_chat_endpoint: None,
            copilot_models_endpoint: Some(models_url),
        },
    )
    .unwrap();
    set_provider_models_cache_dir_for_tests(Some(cache_dir.clone()));
    let mut cfg = Config::default();
    let cli = Cli {
        provider: Some("github-copilot".into()),
        model: None,
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: None,
        command: None,
    };

    let refresh = refresh_model_cache(&cli, &cfg, &store).await;
    refresh.unwrap();
    apply_overrides(&mut cfg, &cli).unwrap();
    set_provider_models_cache_dir_for_tests(None);
    let _ = std::fs::remove_dir_all(cache_dir);

    assert_eq!(cfg.provider, "github-copilot");
    assert_eq!(cfg.model, "copilot-api-model");
    assert_eq!(cfg.auth, "github-copilot");
}

#[tokio::test]
async fn cli_github_copilot_model_alias_refreshes_empty_cache() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let models_url = format!("http://{}/models", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 1024];
        let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
            .await
            .unwrap();
        let body = r#"{"data":[{"id":"copilot-api-model"}]}"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, reply.as_bytes())
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::shutdown(&mut stream)
            .await
            .unwrap();
    });
    let cache_dir = unique_cache_dir("github-copilot-alias-refresh");
    let store = MemoryCredentialStore::default();
    save_github_copilot_tokens(
        &store,
        &GitHubCopilotTokens {
            github_access_token: "github".into(),
            github_refresh_token: None,
            github_expires_at_unix: None,
            copilot_token: Some("copilot-test-token".into()),
            copilot_expires_at_unix: Some(i64::MAX),
            copilot_refresh_after_unix: None,
            copilot_token_endpoint: None,
            copilot_chat_endpoint: None,
            copilot_models_endpoint: Some(models_url),
        },
    )
    .unwrap();
    set_provider_models_cache_dir_for_tests(Some(cache_dir.clone()));
    let mut cfg = Config {
        model_aliases: aliases(&[("copilot", "github-copilot/copilot-api-model")]),
        ..Config::default()
    };
    let cli = Cli {
        provider: None,
        model: Some("@copilot".into()),
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: None,
        command: None,
    };

    refresh_model_cache(&cli, &cfg, &store).await.unwrap();
    apply_overrides(&mut cfg, &cli).unwrap();
    set_provider_models_cache_dir_for_tests(None);
    let _ = std::fs::remove_dir_all(cache_dir);

    assert_eq!(cfg.provider, "github-copilot");
    assert_eq!(cfg.model, "copilot-api-model");
    assert_eq!(cfg.auth, "github-copilot");
    assert_eq!(cfg.current_model_alias(), Some("copilot"));
}

#[test]
fn cli_github_copilot_provider_override_uses_cached_default() {
    with_cached_provider_models("github-copilot", vec!["copilot-cached-model"], || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: Some("github-copilot".into()),
            model: None,
            config: None,
            auth: None,
            no_system_prompt: false,
            no_tools: false,
            no_subagents: false,
            agent: None,
            reasoning: None,
            resume: None,
            command: None,
        };

        apply_overrides(&mut cfg, &cli).unwrap();

        assert_eq!(cfg.provider, "github-copilot");
        assert_eq!(cfg.model, "copilot-cached-model");
        assert_eq!(cfg.auth, "github-copilot");
    });
}

#[test]
fn cli_github_copilot_model_override_selects_matching_auth() {
    with_cached_provider_models("github-copilot", vec!["gpt-4.1"], || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: None,
            model: Some("github-copilot/gpt-4.1".into()),
            config: None,
            auth: None,
            no_system_prompt: false,
            no_tools: false,
            no_subagents: false,
            agent: None,
            reasoning: None,
            resume: None,
            command: None,
        };

        apply_overrides(&mut cfg, &cli).unwrap();

        assert_eq!(cfg.provider, "github-copilot");
        assert_eq!(cfg.model, "gpt-4.1");
        assert_eq!(cfg.auth, "github-copilot");
    });
}

#[test]
fn cli_explicit_provider_keeps_slash_containing_model_id() {
    with_cached_provider_models("openrouter", vec!["anthropic/claude-sonnet-4"], || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: Some("openrouter".into()),
            model: Some("anthropic/claude-sonnet-4".into()),
            config: None,
            auth: None,
            no_system_prompt: false,
            no_tools: false,
            no_subagents: false,
            agent: None,
            reasoning: None,
            resume: None,
            command: None,
        };

        apply_overrides(&mut cfg, &cli).unwrap();

        assert_eq!(cfg.provider, "openrouter");
        assert_eq!(cfg.model, "anthropic/claude-sonnet-4");
        assert_eq!(cfg.auth, "openrouter-api-key");
    });
}

#[test]
fn cli_unqualified_model_override_keeps_provider_for_allowlisted_model() {
    let mut cfg = Config {
        provider: "openai-codex".into(),
        auth: "codex".into(),
        ..Config::default()
    };
    let cli = Cli {
        provider: None,
        model: Some("gpt-5.4-mini".into()),
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: None,
        command: None,
    };

    apply_overrides(&mut cfg, &cli).unwrap();

    assert_eq!(cfg.provider, "openai-codex");
    assert_eq!(cfg.model, "gpt-5.4-mini");
    assert_eq!(cfg.auth, "codex");
}

#[test]
fn cli_auth_override_wins_after_model_provider_auth() {
    let mut cfg = Config::default();
    let cli = Cli {
        provider: None,
        model: Some("openai-codex/gpt-5.4-mini".into()),
        config: None,
        auth: Some("api-key".into()),
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: None,
        command: None,
    };

    apply_overrides(&mut cfg, &cli).unwrap();

    assert_eq!(cfg.provider, "openai");
    assert_eq!(cfg.model, "gpt-5.4-mini");
    assert_eq!(cfg.auth, "api-key");
}

#[test]
fn cli_reasoning_override_updates_config() {
    let mut cfg = Config::default();
    let cli = Cli {
        provider: None,
        model: None,
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: Some(rho_providers::reasoning::ReasoningLevel::High),
        resume: None,
        command: None,
    };

    let save_config = apply_overrides(&mut cfg, &cli).unwrap();

    assert!(save_config);
    assert_eq!(
        cfg.reasoning,
        rho_providers::reasoning::ReasoningLevel::High
    );
}

#[test]
fn authenticated_kimi_capabilities_normalize_stored_reasoning_without_disabling_it() {
    let cache_dir = unique_cache_dir("kimi-normalization");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models_for_tests(
            "kimi-code",
            &[ProviderModel {
                provider: "kimi-code".into(),
                model: "k3".into(),
                display_name: "Kimi K3".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Levels(ReasoningLevelSet::new(
                    vec![
                        rho_sdk::ReasoningLevel::Off,
                        rho_sdk::ReasoningLevel::Low,
                        rho_sdk::ReasoningLevel::High,
                        rho_sdk::ReasoningLevel::Max,
                    ],
                )),
            }],
        )
        .unwrap();
        let mut config = Config {
            provider: "kimi-code".into(),
            model: "k3".into(),
            reasoning: rho_sdk::ReasoningLevel::Medium,
            ..Config::default()
        };

        assert!(normalize_reasoning(&mut config));
        assert_eq!(config.reasoning, rho_sdk::ReasoningLevel::High);

        config.reasoning = rho_sdk::ReasoningLevel::Off;
        assert!(!normalize_reasoning(&mut config));
        assert_eq!(config.reasoning, rho_sdk::ReasoningLevel::Off);
    });
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn explicit_kimi_reasoning_is_preserved_without_authenticated_capabilities() {
    let cache_dir = unique_cache_dir("kimi-explicit-unknown");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models_for_tests(
            "kimi-code",
            &[ProviderModel {
                provider: "kimi-code".into(),
                model: "k3".into(),
                display_name: "Kimi K3".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }],
        )
        .unwrap();
        let mut config = Config {
            provider: "kimi-code".into(),
            model: "k3".into(),
            reasoning: rho_sdk::ReasoningLevel::Low,
            ..Config::default()
        };

        assert!(!normalize_reasoning_for_cli(
            &mut config,
            rho_providers::model::ReasoningRequestSource::Explicit,
        )
        .unwrap());
        assert_eq!(config.reasoning, rho_sdk::ReasoningLevel::Low);
    });
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn explicit_known_unsupported_reasoning_is_rejected() {
    let cache_dir = unique_cache_dir("kimi-explicit-unsupported");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models_for_tests(
            "kimi-code",
            &[ProviderModel {
                provider: "kimi-code".into(),
                model: "k3".into(),
                display_name: "Kimi K3".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Levels(ReasoningLevelSet::new(
                    vec![
                        rho_sdk::ReasoningLevel::Off,
                        rho_sdk::ReasoningLevel::Low,
                        rho_sdk::ReasoningLevel::High,
                        rho_sdk::ReasoningLevel::Max,
                    ],
                )),
            }],
        )
        .unwrap();
        let mut config = Config {
            provider: "kimi-code".into(),
            model: "k3".into(),
            reasoning: rho_sdk::ReasoningLevel::Medium,
            ..Config::default()
        };

        let error = normalize_reasoning_for_cli(
            &mut config,
            rho_providers::model::ReasoningRequestSource::Explicit,
        )
        .expect_err("known unsupported reasoning should fail");

        assert!(error
            .to_string()
            .contains("does not support reasoning level 'medium'"));
        assert_eq!(config.reasoning, rho_sdk::ReasoningLevel::Medium);
    });
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn not_configurable_models_retain_persisted_preference_and_reject_explicit_control() {
    let cache_dir = unique_cache_dir("kimi-not-configurable");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        replace_cached_provider_models_for_tests(
            "kimi-code",
            &[ProviderModel {
                provider: "kimi-code".into(),
                model: "fixed".into(),
                display_name: "Fixed".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::NotConfigurable,
            }],
        )
        .unwrap();
        let mut config = Config {
            provider: "kimi-code".into(),
            model: "fixed".into(),
            reasoning: rho_sdk::ReasoningLevel::High,
            ..Config::default()
        };

        assert!(!normalize_reasoning(&mut config));
        assert_eq!(config.reasoning, rho_sdk::ReasoningLevel::High);
        let error = normalize_reasoning_for_cli(
            &mut config,
            rho_providers::model::ReasoningRequestSource::Explicit,
        )
        .expect_err("fixed models should reject an explicit reasoning control");
        assert!(error
            .to_string()
            .contains("does not expose configurable reasoning"));
    });
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn only_kimi_prepares_provider_capabilities_during_startup() {
    let refresh = super::ProviderRefreshStatus::NotAttempted;
    let xai = Config {
        provider: "xai".into(),
        model: "unseen-model".into(),
        ..Config::default()
    };
    let kimi = Config {
        provider: "kimi-code".into(),
        model: "unseen-model".into(),
        ..Config::default()
    };

    assert!(!super::needs_startup_capability_refresh(&xai, &refresh));
    assert!(super::needs_startup_capability_refresh(&kimi, &refresh));
}

#[test]
fn only_kimi_requires_synchronous_capability_discovery() {
    assert!(!super::needs_synchronous_capability_refresh(
        "xai",
        "unseen-model"
    ));
    assert!(super::needs_synchronous_capability_refresh(
        "kimi-code",
        "unseen-model"
    ));
}

#[test]
fn refresh_selection_uses_loaded_config_without_cli_model_flags() {
    let config = Config {
        provider: "kimi-code".into(),
        model: "k3".into(),
        ..Config::default()
    };

    assert_eq!(
        super::selected_model_for_refresh(&config, "kimi-code"),
        Some("k3".into())
    );
}
fn aliases(pairs: &[(&str, &str)]) -> crate::model_aliases::ModelAliases {
    crate::model_aliases::ModelAliases::from_entries(
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    )
    .unwrap()
}

#[test]
fn cli_model_override_resolves_user_defined_alias() {
    with_cached_provider_models("anthropic", vec!["claude-sonnet-4-5"], || {
        let mut cfg = Config {
            model_aliases: aliases(&[("deep", "anthropic/claude-sonnet-4-5")]),
            ..Config::default()
        };
        let cli = Cli {
            provider: None,
            model: Some("@deep".into()),
            config: None,
            auth: None,
            no_system_prompt: false,
            no_tools: false,
            no_subagents: false,
            agent: None,
            reasoning: None,
            resume: None,
            command: None,
        };

        let save_config = apply_overrides(&mut cfg, &cli).unwrap();

        assert!(save_config);
        assert_eq!(cfg.provider, "anthropic");
        assert_eq!(cfg.model, "claude-sonnet-4-5");
        assert_eq!(cfg.current_model_alias(), Some("deep"));
    });
}

#[test]
fn cli_model_alias_conflicting_with_provider_flag_errors() {
    let mut cfg = Config {
        model_aliases: aliases(&[("deep", "openai/gpt-5.5")]),
        ..Config::default()
    };
    let cli = Cli {
        provider: Some("anthropic".into()),
        model: Some("@deep".into()),
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: None,
        command: None,
    };

    let error = apply_overrides(&mut cfg, &cli).unwrap_err();

    assert!(
        error.to_string().contains(
            "model alias '@deep' resolves to provider 'openai', which conflicts with --provider anthropic"
        ),
        "{error:#}"
    );
}

#[test]
fn undefined_cli_model_alias_names_flag() {
    let mut cfg = Config::default();
    let cli = Cli {
        provider: None,
        model: Some("@missing".into()),
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: None,
        command: None,
    };

    let error = apply_overrides(&mut cfg, &cli).unwrap_err();

    assert!(
        error.to_string().contains(
            "--model: model alias '@missing' is not defined; define it in [model.aliases] or use a concrete model reference"
        ),
        "{error:#}"
    );
}

#[test]
fn cli_auth_only_selection_resolves_provider_profile() {
    with_cached_provider_models(
        "openrouter-oauth",
        vec!["anthropic/claude-sonnet-4"],
        || {
            let mut cfg = Config::default();
            let cli = Cli {
                provider: None,
                model: None,
                config: None,
                auth: Some("openrouter-oauth".into()),
                no_system_prompt: false,
                no_tools: false,
                no_subagents: false,
                agent: None,
                reasoning: None,
                resume: None,
                command: None,
            };

            apply_overrides(&mut cfg, &cli).unwrap();

            assert_eq!(cfg.provider, "openrouter-oauth");
            assert_eq!(cfg.model, "anthropic/claude-sonnet-4");
            assert_eq!(cfg.auth, "openrouter-oauth");
        },
    );
}

#[test]
fn cli_auth_profile_normalizes_compatible_provider() {
    with_cached_provider_models("openrouter", vec!["anthropic/claude-sonnet-4"], || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: Some("openrouter".into()),
            model: Some("anthropic/claude-sonnet-4".into()),
            config: None,
            auth: Some("openrouter-oauth".into()),
            no_system_prompt: false,
            no_tools: false,
            no_subagents: false,
            agent: None,
            reasoning: None,
            resume: None,
            command: None,
        };

        apply_overrides(&mut cfg, &cli).unwrap();

        assert_eq!(cfg.provider, "openrouter-oauth");
        assert_eq!(cfg.auth, "openrouter-oauth");
    });
}
