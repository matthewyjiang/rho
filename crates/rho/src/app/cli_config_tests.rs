use std::time::{SystemTime, UNIX_EPOCH};

use {
    crate::cli::{Cli, Command},
    crate::config::Config,
    rho_providers::credentials::{
        save_github_copilot_tokens, GitHubCopilotTokens, MemoryCredentialStore,
    },
    rho_providers::model::provider_models::{
        replace_cached_provider_models_for_tests, set_provider_models_cache_dir_for_tests,
        with_provider_models_cache_dir_for_tests, ProviderModel,
    },
};

use super::{apply_overrides, refresh_model_cache, validate};

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

    let refresh = refresh_model_cache(&cli, &store).await;
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

    assert_eq!(cfg.provider, "openai-codex");
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
