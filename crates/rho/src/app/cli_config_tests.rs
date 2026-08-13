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
            cached_provider_models, replace_cached_provider_models_for_tests,
            set_provider_models_cache_dir_for_tests, with_provider_models_cache_dir_for_tests,
            ProviderModel,
        },
        ReasoningCapabilities, ReasoningLevelSet,
    },
};

use super::{
    apply_overrides, normalize_reasoning, normalize_reasoning_for_cli,
    refresh_custom_provider_models, refresh_model_cache, validate,
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

fn aliases(pairs: &[(&str, &str)]) -> crate::model_aliases::ModelAliases {
    crate::model_aliases::ModelAliases::from_entries(
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    )
    .unwrap()
}

fn test_cli() -> Cli {
    Cli {
        provider: None,
        model: None,
        config: None,
        auth: None,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        permission_mode: None,
        save: false,
        resume: None,
        command: None,
    }
}

// Covers: --resume is rejected for non-interactive commands before prompt work starts
// Owner: cli config validation
#[test]
fn validate_cli_rejects_resume_with_non_interactive_commands() {
    for command in [
        Command::Run {
            stdin: true,
            output_file: None,
            output: crate::cli::OutputFormat::Text,
            max_steps: None,
            timeout: None,
            prompt: Vec::new(),
        },
        Command::Update,
    ] {
        let cli = Cli {
            resume: Some(Some("session-id".into())),
            command: Some(command),
            ..test_cli()
        };

        let err = validate(&cli).unwrap_err();
        assert!(err.to_string().contains("--resume is only supported"));
    }
}

#[test]
fn poolside_model_override_persists_internal_model_id() {
    with_cached_provider_models("poolside", vec!["laguna-m.1"], || {
        let mut config = Config::default();
        let cli = Cli::try_parse_from(["rho", "--model", "poolside/laguna-m.1"]).unwrap();

        assert!(apply_overrides(&mut config, &cli).unwrap());
        assert_eq!(config.provider, "poolside");
        assert_eq!(config.model, "laguna-m.1");
        assert_eq!(config.auth, "poolside-api-key");
    });
}

#[test]
fn legacy_xai_provider_override_normalizes_to_oauth_mode() {
    let mut config = Config::default();
    let cli = Cli::try_parse_from(["rho", "--provider", "xai-oauth"]).unwrap();

    assert!(apply_overrides(&mut config, &cli).unwrap());
    assert_eq!(config.provider, "xai");
    assert_eq!(config.model, "grok-4.6");
    assert_eq!(config.auth, "xai-oauth");
}

#[test]
fn cli_model_override_with_provider_selects_matching_auth() {
    let mut cfg = Config::default();
    let cli = Cli {
        model: Some("openai-codex/gpt-5.4-mini".into()),
        ..test_cli()
    };

    let changed = apply_overrides(&mut cfg, &cli).unwrap();

    assert!(changed);
    assert_eq!(cfg.provider, "openai-codex");
    assert_eq!(cfg.model, "gpt-5.4-mini");
    assert_eq!(cfg.auth, "codex");
}

// Covers: identical override flags do not count as a config change for --save
// Owner: cli config overrides
#[test]
fn identical_cli_overrides_do_not_report_change() {
    let mut cfg = Config::default();
    cfg.normalize_provider_profiles().unwrap();
    let reasoning = cfg.reasoning;
    let cli = Cli {
        reasoning: Some(reasoning),
        ..test_cli()
    };

    assert!(!apply_overrides(&mut cfg, &cli).unwrap());
    assert_eq!(cfg.reasoning, reasoning);
}

// Covers: --permission-mode overrides config for the invocation without counting as --save change
// Owner: cli config overrides
#[test]
fn cli_permission_mode_override_applies_without_reporting_change() {
    let mut cfg = Config::default();
    let cli = Cli::try_parse_from(["rho", "--permission-mode", "auto"]).unwrap();

    assert!(!apply_overrides(&mut cfg, &cli).unwrap());
    assert_eq!(cfg.permission_mode, crate::permission::PermissionMode::Auto);
}

#[test]
fn cli_anthropic_provider_override_without_cache_uses_builtin_default() {
    let cache_dir = unique_cache_dir("anthropic-empty");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: Some("anthropic".into()),
            ..test_cli()
        };

        apply_overrides(&mut cfg, &cli).unwrap();

        assert_eq!(cfg.provider, "anthropic");
        assert_eq!(cfg.model, "claude-sonnet-4-5");
        assert_eq!(cfg.auth, "anthropic-api-key");
    });
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn cli_github_copilot_provider_override_requires_cached_default() {
    let cache_dir = unique_cache_dir("github-copilot-empty");
    with_provider_models_cache_dir_for_tests(cache_dir.clone(), || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: Some("github-copilot".into()),
            ..test_cli()
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
        ..test_cli()
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

#[test]
fn cli_github_copilot_provider_override_uses_cached_default() {
    with_cached_provider_models("github-copilot", vec!["copilot-cached-model"], || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: Some("github-copilot".into()),
            ..test_cli()
        };

        apply_overrides(&mut cfg, &cli).unwrap();

        assert_eq!(cfg.provider, "github-copilot");
        assert_eq!(cfg.model, "copilot-cached-model");
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
            ..test_cli()
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
        model: Some("gpt-5.4-mini".into()),
        ..test_cli()
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
        model: Some("openai-codex/gpt-5.4-mini".into()),
        auth: Some("api-key".into()),
        ..test_cli()
    };

    apply_overrides(&mut cfg, &cli).unwrap();

    assert_eq!(cfg.provider, "openai");
    assert_eq!(cfg.model, "gpt-5.4-mini");
    assert_eq!(cfg.auth, "api-key");
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
fn only_capability_backed_providers_prepare_capabilities_during_startup() {
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
    let anthropic = Config {
        provider: "anthropic".into(),
        model: "claude-opus-5".into(),
        ..Config::default()
    };

    assert!(!super::needs_startup_capability_refresh(&xai, &refresh));
    assert!(super::needs_startup_capability_refresh(&kimi, &refresh));
    assert!(super::needs_startup_capability_refresh(
        &anthropic, &refresh
    ));
}

#[test]
fn cli_model_override_resolves_user_defined_alias() {
    with_cached_provider_models("anthropic", vec!["claude-sonnet-4-5"], || {
        let mut cfg = Config {
            model_aliases: aliases(&[("deep", "anthropic/claude-sonnet-4-5")]),
            ..Config::default()
        };
        let cli = Cli {
            model: Some("@deep".into()),
            ..test_cli()
        };

        let changed = apply_overrides(&mut cfg, &cli).unwrap();

        assert!(changed);
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
        ..test_cli()
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
        model: Some("@missing".into()),
        ..test_cli()
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
    with_cached_provider_models("openrouter", vec!["anthropic/claude-sonnet-4"], || {
        let mut cfg = Config::default();
        let cli = Cli {
            auth: Some("openrouter-oauth".into()),
            ..test_cli()
        };

        apply_overrides(&mut cfg, &cli).unwrap();

        assert_eq!(cfg.provider, "openrouter");
        assert_eq!(cfg.model, "anthropic/claude-sonnet-4");
        assert_eq!(cfg.auth, "openrouter-oauth");
    });
}

#[test]
fn cli_auth_profile_normalizes_compatible_provider() {
    with_cached_provider_models("openrouter", vec!["anthropic/claude-sonnet-4"], || {
        let mut cfg = Config::default();
        let cli = Cli {
            provider: Some("openrouter".into()),
            model: Some("anthropic/claude-sonnet-4".into()),
            auth: Some("openrouter-oauth".into()),
            ..test_cli()
        };

        apply_overrides(&mut cfg, &cli).unwrap();

        assert_eq!(cfg.provider, "openrouter");
        assert_eq!(cfg.auth, "openrouter-oauth");
    });
}

// Covers: custom hosts must populate the picker from /v1/models without a manual refresh
// Owner: app startup
// The registry lock must cover the whole test so parallel suites cannot mutate the
// process-wide custom provider set while this current-thread test is awaiting I/O.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "current_thread")]
async fn custom_hosts_fetch_models_from_the_openai_compatible_endpoint() {
    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    struct RestoreCustomProviders;
    impl Drop for RestoreCustomProviders {
        fn drop(&mut self) {
            rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 2048];
        let bytes = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
            .await
            .unwrap();
        let request = String::from_utf8_lossy(&buffer[..bytes]);
        assert!(request.starts_with("GET /v1/models HTTP/1.1"));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        let body = r#"{"data":[{"id":"composer-2.5"},{"id":"composer-2.5-fast"}]}"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, reply.as_bytes())
            .await
            .unwrap();
    });

    let cache_dir = unique_cache_dir("custom-host-refresh");
    set_provider_models_cache_dir_for_tests(Some(cache_dir.clone()));
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    let _restore = RestoreCustomProviders;
    let mut cfg = Config::default();
    cfg.providers.set_endpoint("composer", &api_base).unwrap();
    cfg.providers.activate().unwrap();
    let store = MemoryCredentialStore::default();

    refresh_custom_provider_models(&cfg, &store).await;
    let models = cached_provider_models("composer");
    set_provider_models_cache_dir_for_tests(None);
    let _ = std::fs::remove_dir_all(cache_dir);

    assert_eq!(
        models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        ["composer-2.5", "composer-2.5-fast"]
    );
}

// Covers: unavailable custom hosts must not serialize their discovery timeouts
// Owner: app startup
// Same process-wide registry lock as the fetch test above.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "current_thread")]
async fn custom_hosts_refresh_models_concurrently() {
    let _lock = rho_providers::provider::custom_provider_registry_test_lock();
    struct RestoreCustomProviders;
    impl Drop for RestoreCustomProviders {
        fn drop(&mut self) {
            rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
        }
    }

    let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_a = format!("http://{}/v1", listener_a.local_addr().unwrap());
    let api_b = format!("http://{}/v1", listener_b.local_addr().unwrap());
    let (ready_a, wait_a) = tokio::sync::oneshot::channel();
    let (ready_b, wait_b) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);

    let spawn_hold = |listener: tokio::net::TcpListener,
                      ready: tokio::sync::oneshot::Sender<()>,
                      mut release_rx: tokio::sync::watch::Receiver<bool>| {
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer)
                .await
                .unwrap();
            let _ = ready.send(());
            release_rx.wait_for(|released| *released).await.unwrap();
            let body = r#"{"data":[{"id":"m"}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, reply.as_bytes()).await;
        })
    };
    spawn_hold(listener_a, ready_a, release_rx.clone());
    spawn_hold(listener_b, ready_b, release_rx);

    let cache_dir = unique_cache_dir("custom-host-concurrent");
    set_provider_models_cache_dir_for_tests(Some(cache_dir.clone()));
    rho_providers::provider::reset_custom_openai_compatible_providers_for_tests();
    let _restore = RestoreCustomProviders;
    let mut cfg = Config::default();
    cfg.providers.set_endpoint("composer", &api_a).unwrap();
    cfg.providers.set_endpoint("vllm", &api_b).unwrap();
    cfg.providers.activate().unwrap();
    let store = MemoryCredentialStore::default();

    let refresh = tokio::spawn(async move {
        refresh_custom_provider_models(&cfg, &store).await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        wait_a.await.unwrap();
        wait_b.await.unwrap();
    })
    .await
    .expect("both custom hosts should accept before either responds");
    release_tx.send(true).unwrap();
    refresh.await.unwrap();
    set_provider_models_cache_dir_for_tests(None);
    let _ = std::fs::remove_dir_all(cache_dir);
}
