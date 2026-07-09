mod agent;
mod auth;
mod cli;
mod clipboard_image;
mod commands;
mod config;
mod credentials;
mod herdr;
mod model;
mod paths;
mod prompt;
mod provider_backend;
mod reasoning;
mod session;
mod skills;
mod tool;
mod tools;
mod transcript;
mod tui;
mod update;
mod workspace;

use std::io::{self, IsTerminal, Read};

use clap::Parser;

use crate::{
    agent::{Agent, SessionHistorySink},
    auth::{codex_oauth, github_copilot_device},
    cli::{Cli, Command},
    config::Config,
    credentials::{save_codex_tokens, save_github_copilot_tokens, OsCredentialStore},
    herdr::{HerdrReporter, HerdrState},
    model::{
        build_provider, catalog, models_dev::cached_model_metadata,
        provider_models::refresh_provider_models_with_store, registry, ModelError,
        UnavailableProvider,
    },
    session::Session,
    tool::{ToolContext, ToolRegistry},
    tui::TuiInfo,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    validate_cli(&cli)?;
    if matches!(cli.command, Some(Command::Update)) {
        return update::run_update(env!("CARGO_PKG_VERSION")).await;
    }
    if let Some(Command::Login {
        provider,
        device_auth,
    }) = &cli.command
    {
        return run_login_command(provider, *device_auth).await;
    }

    let config_path = cli.config.clone();
    let mut cfg = Config::load(config_path.clone())?;
    let store = OsCredentialStore;
    refresh_cli_model_cache(&cli, &store).await?;
    let save_config = apply_cli_overrides(&mut cfg, &cli)?;
    if save_config {
        cfg.save(config_path.clone())?;
    }

    if cli.command.is_none() && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
        anyhow::bail!(
            "rho's default mode is the interactive TUI; use `rho run` for non-interactive automation"
        );
    }
    let run_prompt = match &cli.command {
        Some(Command::Run { prompt, stdin }) => Some(automation_prompt(prompt.clone(), *stdin)?),
        Some(Command::Login { .. }) | Some(Command::Update) | None => None,
    };
    let update_notice = if cli.command.is_none() && cfg.check_for_updates {
        update::update_notice(env!("CARGO_PKG_VERSION")).await
    } else {
        None
    };

    if cfg.provider == "anthropic" && cached_model_metadata(&cfg.provider, &cfg.model).is_none() {
        let _ = model::models_dev::fetch_model_metadata(&cfg.provider, &cfg.model).await;
    }

    let provider_result = build_provider(&cfg.provider, &cfg.model, cfg.reasoning);
    let missing_auth_error = provider_result
        .as_ref()
        .err()
        .filter(|err| is_interactive_startup_unavailable_error(err))
        .map(model_error_message);
    let provider = match provider_result {
        Ok(provider) => provider,
        Err(err) if run_prompt.is_none() && is_interactive_startup_unavailable_error(&err) => {
            Box::new(UnavailableProvider::new(err))
        }
        Err(err) => return Err(err.into()),
    };
    let registry = if cli.no_tools {
        ToolRegistry::new()
    } else {
        tools::registry(&cfg)
    };
    let cwd = std::env::current_dir()?;
    let ctx = ToolContext {
        cwd: cwd.clone(),
        max_output_bytes: cfg.max_output_bytes,
    };
    let herdr = HerdrReporter::from_env();
    let mut agent = Agent::new(provider, registry, ctx);
    if cli.no_system_prompt {
        agent = agent.without_system_prompt();
    }
    agent.set_compaction_config((&cfg).into());
    agent.set_context_window(
        cached_model_metadata(&cfg.provider, &cfg.model)
            .and_then(|metadata| metadata.display_context_window()),
    );

    match run_prompt {
        Some(prompt) => {
            herdr.report_state(HerdrState::Working, None, None).await;
            let result = agent.run(prompt).await;
            herdr.report_state(HerdrState::Idle, None, None).await;
            herdr.release().await;
            let answer = result?;
            println!("{answer}");
        }
        None => {
            let mut open_resume_picker = false;
            let mut recovered_messages = Vec::new();
            let session_id = match &cli.resume {
                Some(Some(id)) => {
                    let (session, histories) = Session::open_by_id_with_histories(&cwd, id)?;
                    let session_id = Some(session.id().to_string());
                    recovered_messages = histories.display;
                    agent = agent.with_history(histories.model);
                    agent.set_session_id(session_id.clone());
                    agent.set_history_sink(SessionHistorySink::new(session));
                    session_id
                }
                Some(None) => {
                    open_resume_picker = true;
                    None
                }
                None => None,
            };
            let tui_result = tui::run(
                &mut agent,
                TuiInfo {
                    cwd,
                    provider: cfg.provider,
                    model: cfg.model,
                    reasoning: cfg.reasoning,
                    show_reasoning_output: cfg.show_reasoning_output,
                    auth: cfg.auth,
                    title_provider: cfg.title_provider,
                    title_model: cfg.title_model,
                    title_auth: cfg.title_auth,
                    favorite_models: cfg.favorite_models,
                    max_tool_output_lines: cfg.max_tool_output_lines,
                    questionnaire_enabled: !cli.no_tools,
                    session_id,
                    recovered_messages,
                    open_resume_picker,
                    config_path,
                    auth_unavailable: missing_auth_error,
                    update_notice,
                    herdr,
                },
            )
            .await?;
            if let Some(session_id) = tui_result.resume_session_id {
                println!("\nResume this session:\n  rho --resume {session_id}\n");
            }
        }
    }
    Ok(())
}

async fn run_login_command(provider: &str, device_auth: bool) -> anyhow::Result<()> {
    let Some(target) = catalog::login_target_for_provider(provider) else {
        anyhow::bail!(
            "unsupported login provider '{provider}'. Use one of: {}",
            catalog::implemented_providers().join(", ")
        );
    };
    let Some(descriptor) = registry::provider_descriptor(&target.provider) else {
        anyhow::bail!("unsupported login provider '{}'", target.provider);
    };
    let store = OsCredentialStore;

    match descriptor.auth_kind {
        registry::ProviderAuthKind::CodexOAuth { .. } => {
            let tokens = if device_auth {
                let login = codex_oauth::start_codex_device_login().await?;
                eprintln!(
                    "Codex login: visit {} and enter code {}",
                    login.verification_uri, login.user_code
                );
                codex_oauth::complete_codex_device_login(login).await?
            } else {
                eprintln!("Opening browser for Codex login. On a remote or headless session, use `rho login openai-codex --device-auth` instead.");
                codex_oauth::run_codex_oauth_flow().await?
            };
            save_codex_tokens(&store, &tokens)?;
        }
        registry::ProviderAuthKind::GithubCopilotDevice { .. } => {
            let login = github_copilot_device::start_github_copilot_device_login().await?;
            eprintln!(
                "GitHub Copilot login: visit {} and enter code {}",
                login.verification_uri, login.user_code
            );
            if let Some(uri) = &login.verification_uri_complete {
                eprintln!("Or open this URL to continue: {uri}");
            }
            let tokens = github_copilot_device::complete_github_copilot_device_login(login).await?;
            save_github_copilot_tokens(&store, &tokens)?;
        }
        registry::ProviderAuthKind::ApiKey { entry_label, .. } => {
            anyhow::bail!(
                "{entry_label} login is only supported in the interactive TUI; run `/login {provider}`"
            );
        }
    }

    eprintln!("Successfully logged in to {}", target.provider);
    Ok(())
}

fn is_interactive_startup_unavailable_error(error: &ModelError) -> bool {
    matches!(
        error,
        ModelError::MissingApiKey
            | ModelError::MissingCodexAuth
            | ModelError::MissingAnthropicApiKey
            | ModelError::MissingGithubCopilotAuth
            | ModelError::Credentials(_)
            | ModelError::UnsupportedProvider(_)
    )
}

fn model_error_message(error: &ModelError) -> String {
    error.to_string()
}

fn validate_cli(cli: &Cli) -> anyhow::Result<()> {
    if cli.resume.is_some() && cli.command.is_some() {
        anyhow::bail!("--resume is only supported for interactive sessions");
    }
    Ok(())
}

async fn refresh_cli_model_cache(
    cli: &Cli,
    store: &dyn credentials::CredentialStore,
) -> anyhow::Result<()> {
    let provider = cli
        .provider
        .as_deref()
        .or_else(|| cli.model.as_deref().and_then(explicit_model_provider));
    if let Some(provider) = provider {
        refresh_model_list_for_cli_provider(provider, store).await?;
    }
    Ok(())
}

fn apply_cli_overrides(cfg: &mut Config, cli: &Cli) -> anyhow::Result<bool> {
    let mut save_config = false;
    if let Some(provider) = &cli.provider {
        apply_provider_override(cfg, provider, cli.model.is_some())?;
        save_config = true;
    }
    if let Some(model) = &cli.model {
        apply_model_override(cfg, model)?;
        save_config = true;
    }
    if let Some(auth) = &cli.auth {
        cfg.auth = auth.clone();
        save_config = true;
    }
    if let Some(reasoning) = cli.reasoning {
        cfg.reasoning = reasoning;
        save_config = true;
    }
    let reasoning = cfg.reasoning.for_model(&cfg.provider, &cfg.model);
    if reasoning != cfg.reasoning {
        cfg.reasoning = reasoning;
        save_config = true;
    }
    Ok(save_config)
}

fn apply_provider_override(
    cfg: &mut Config,
    provider: &str,
    has_model_override: bool,
) -> anyhow::Result<()> {
    if !catalog::implemented_providers().contains(&provider) {
        anyhow::bail!("unknown provider '{provider}' for --provider");
    }
    let auth = catalog::login_target_for_provider(provider).map(|target| target.auth);
    let model = if has_model_override {
        None
    } else {
        Some(catalog::default_model_for_provider(provider).ok_or_else(|| {
            anyhow::anyhow!(
                "no cached models for provider '{provider}'. Run /refresh-model-list {provider} or pass a cached provider/model with --model"
            )
        })?)
    };
    cfg.provider = provider.to_string();
    if let Some(auth) = auth {
        cfg.auth = auth;
    }
    if let Some(model) = model {
        cfg.model = model;
    }
    Ok(())
}

fn apply_model_override(cfg: &mut Config, model: &str) -> anyhow::Result<()> {
    let selection = catalog::resolve_model_selection_for_auths(
        model,
        &cfg.provider,
        &cfg.auth,
        std::slice::from_ref(&cfg.auth),
    )?;
    cfg.provider = selection.provider;
    cfg.model = selection.model;
    cfg.auth = selection.auth;
    Ok(())
}

async fn refresh_model_list_for_cli_provider(
    provider: &str,
    store: &dyn credentials::CredentialStore,
) -> anyhow::Result<()> {
    let Some(descriptor) = registry::provider_descriptor(provider) else {
        return Ok(());
    };
    if descriptor.model_refresh.is_none() || catalog::default_model_for_provider(provider).is_some()
    {
        return Ok(());
    }
    match refresh_provider_models_with_store(provider, store).await {
        Ok(_) => Ok(()),
        Err(err) if provider_requires_cached_models(provider) => Err(err.into()),
        Err(_) => Ok(()),
    }
}

fn provider_requires_cached_models(provider: &str) -> bool {
    registry::provider_descriptor(provider)
        .map(|descriptor| {
            descriptor.model_source == registry::ProviderModelSource::CachedProviderModels
        })
        .unwrap_or(false)
}

fn explicit_model_provider(model: &str) -> Option<&str> {
    let (provider, model) = model.trim().split_once('/')?;
    (!provider.trim().is_empty() && !model.trim().is_empty()).then_some(provider.trim())
}

fn automation_prompt(parts: Vec<String>, read_stdin: bool) -> anyhow::Result<String> {
    automation_prompt_with_stdin(parts, read_stdin, &mut io::stdin())
}

fn automation_prompt_with_stdin(
    parts: Vec<String>,
    read_stdin: bool,
    stdin: &mut impl Read,
) -> anyhow::Result<String> {
    let mut chunks = Vec::new();
    let inline = parts.join(" ").trim().to_string();
    if !inline.is_empty() {
        chunks.push(inline);
    }
    if read_stdin {
        let mut buffer = String::new();
        stdin.read_to_string(&mut buffer)?;
        let buffer = buffer.trim().to_string();
        if !buffer.is_empty() {
            chunks.push(buffer);
        }
    }

    let prompt = chunks.join("\n\n");
    if prompt.is_empty() {
        anyhow::bail!("rho run requires a prompt argument or --stdin");
    }
    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::{
        credentials::{save_github_copilot_tokens, GitHubCopilotTokens, MemoryCredentialStore},
        model::provider_models::{
            replace_cached_provider_models_for_tests, set_provider_models_cache_dir_for_tests,
            with_provider_models_cache_dir_for_tests, ProviderModel,
        },
    };

    fn unique_cache_dir(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should be after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("rho-main-{name}-{}-{nanos}", std::process::id()))
    }

    fn with_cached_provider_models<T>(
        provider: &str,
        models: Vec<&str>,
        f: impl FnOnce() -> T,
    ) -> T {
        let cache_dir = unique_cache_dir(provider);
        let provider_models = models
            .into_iter()
            .map(|model| ProviderModel {
                provider: provider.into(),
                model: model.into(),
                display_name: model.into(),
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
    fn unsupported_provider_is_nonfatal_for_interactive_startup() {
        assert!(is_interactive_startup_unavailable_error(
            &ModelError::UnsupportedProvider("anthropic".into())
        ));
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
            reasoning: None,
            resume: Some(Some("session-id".into())),
            command: Some(Command::Run {
                stdin: true,
                prompt: Vec::new(),
            }),
        };

        let err = validate_cli(&cli).unwrap_err();

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
            reasoning: None,
            resume: Some(Some("session-id".into())),
            command: Some(Command::Update),
        };

        let err = validate_cli(&cli).unwrap_err();

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
            reasoning: None,
            resume: None,
            command: None,
        };

        let save_config = apply_cli_overrides(&mut cfg, &cli).unwrap();

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
                reasoning: None,
                resume: None,
                command: None,
            };

            let save_config = apply_cli_overrides(&mut cfg, &cli).unwrap();

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
                reasoning: None,
                resume: None,
                command: None,
            };

            let save_config = apply_cli_overrides(&mut cfg, &cli).unwrap();

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
            reasoning: None,
            resume: None,
            command: None,
        };

        apply_cli_overrides(&mut cfg, &cli).unwrap();

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
                reasoning: None,
                resume: None,
                command: None,
            };

            let err = apply_cli_overrides(&mut cfg, &cli).unwrap_err();

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
            reasoning: None,
            resume: None,
            command: None,
        };

        let refresh = refresh_cli_model_cache(&cli, &store).await;
        refresh.unwrap();
        apply_cli_overrides(&mut cfg, &cli).unwrap();
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
                reasoning: None,
                resume: None,
                command: None,
            };

            apply_cli_overrides(&mut cfg, &cli).unwrap();

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
                reasoning: None,
                resume: None,
                command: None,
            };

            apply_cli_overrides(&mut cfg, &cli).unwrap();

            assert_eq!(cfg.provider, "github-copilot");
            assert_eq!(cfg.model, "gpt-4.1");
            assert_eq!(cfg.auth, "github-copilot");
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
            reasoning: None,
            resume: None,
            command: None,
        };

        apply_cli_overrides(&mut cfg, &cli).unwrap();

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
            reasoning: None,
            resume: None,
            command: None,
        };

        apply_cli_overrides(&mut cfg, &cli).unwrap();

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
            reasoning: Some(crate::reasoning::ReasoningLevel::High),
            resume: None,
            command: None,
        };

        let save_config = apply_cli_overrides(&mut cfg, &cli).unwrap();

        assert!(save_config);
        assert_eq!(cfg.reasoning, crate::reasoning::ReasoningLevel::High);
    }

    #[test]
    fn cli_reasoning_override_remaps_unsupported_codex_effort() {
        let mut cfg = Config {
            provider: "openai-codex".into(),
            model: "gpt-5.5".into(),
            ..Config::default()
        };
        let cli = Cli {
            provider: None,
            model: None,
            config: None,
            auth: None,
            no_system_prompt: false,
            no_tools: false,
            reasoning: Some(crate::reasoning::ReasoningLevel::Max),
            resume: None,
            command: None,
        };

        let save_config = apply_cli_overrides(&mut cfg, &cli).unwrap();

        assert!(save_config);
        assert_eq!(cfg.reasoning, crate::reasoning::ReasoningLevel::Xhigh);
    }

    #[test]
    fn cli_reasoning_override_keeps_supported_gpt_5_6_effort() {
        let mut cfg = Config {
            provider: "openai-codex".into(),
            model: "gpt-5.6-sol".into(),
            ..Config::default()
        };
        let cli = Cli {
            provider: None,
            model: None,
            config: None,
            auth: None,
            no_system_prompt: false,
            no_tools: false,
            reasoning: Some(crate::reasoning::ReasoningLevel::Max),
            resume: None,
            command: None,
        };

        let save_config = apply_cli_overrides(&mut cfg, &cli).unwrap();

        assert!(save_config);
        assert_eq!(cfg.reasoning, crate::reasoning::ReasoningLevel::Max);
    }

    #[test]
    fn automation_prompt_joins_inline_parts() {
        let mut stdin = io::empty();
        let prompt =
            automation_prompt_with_stdin(vec!["review".into(), "this".into()], false, &mut stdin)
                .unwrap();

        assert_eq!(prompt, "review this");
    }

    #[test]
    fn automation_prompt_combines_inline_and_stdin() {
        let mut stdin = "diff contents".as_bytes();
        let prompt = automation_prompt_with_stdin(vec!["review".into()], true, &mut stdin).unwrap();

        assert_eq!(prompt, "review\n\ndiff contents");
    }

    #[test]
    fn automation_prompt_requires_input() {
        let mut stdin = io::empty();
        let err = automation_prompt_with_stdin(Vec::new(), false, &mut stdin).unwrap_err();

        assert!(err.to_string().contains("requires a prompt"));
    }
}
