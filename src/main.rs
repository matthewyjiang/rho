mod agent;
mod auth;
mod cli;
mod commands;
mod config;
mod credentials;
mod model;
mod paths;
mod prompt;
mod reasoning;
mod session;
mod skills;
mod tool;
mod tools;
mod transcript;
mod tui;
mod workspace;

use std::io::{self, IsTerminal, Read};

use clap::Parser;

use agent::{Agent, SessionHistorySink};
use cli::{Cli, Command};
use config::Config;
use model::{
    build_provider, catalog, models_dev::cached_model_metadata, ModelError, UnavailableProvider,
};
use session::Session;
use tool::{ToolContext, ToolRegistry};
use tui::TuiInfo;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    validate_cli(&cli)?;
    let config_path = cli.config.clone();
    let mut cfg = Config::load(config_path.clone())?;
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
        None => None,
    };

    if cfg.provider == "anthropic" && cached_model_metadata(&cfg.provider, &cfg.model).is_none() {
        let _ = model::models_dev::fetch_model_metadata(&cfg.provider, &cfg.model).await;
    }

    let provider_result = build_provider(&cfg.provider, &cfg.model, cfg.reasoning);
    let missing_auth_error = provider_result
        .as_ref()
        .err()
        .filter(|err| is_auth_unavailable_error(err))
        .map(model_error_message);
    let provider = match provider_result {
        Ok(provider) => provider,
        Err(err) if run_prompt.is_none() && is_auth_unavailable_error(&err) => {
            Box::new(UnavailableProvider::new(err))
        }
        Err(err) => return Err(err.into()),
    };
    let registry = if cli.no_tools {
        ToolRegistry::new()
    } else {
        tools::registry()
    };
    let cwd = std::env::current_dir()?;
    let ctx = ToolContext {
        cwd: cwd.clone(),
        max_output_bytes: cfg.max_output_bytes,
    };
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
            let answer = agent.run(prompt).await?;
            println!("{answer}");
        }
        None => {
            let mut open_resume_picker = false;
            let session_id = match &cli.resume {
                Some(Some(id)) => {
                    let (session, history) = Session::open_by_id(&cwd, id)?;
                    let session_id = Some(session.id().to_string());
                    agent = agent.with_history(history);
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
                    auth: cfg.auth,
                    title_provider: cfg.title_provider,
                    title_model: cfg.title_model,
                    title_auth: cfg.title_auth,
                    max_tool_output_lines: cfg.max_tool_output_lines,
                    session_id,
                    open_resume_picker,
                    config_path,
                    auth_unavailable: missing_auth_error,
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

fn is_auth_unavailable_error(error: &ModelError) -> bool {
    matches!(
        error,
        ModelError::MissingApiKey
            | ModelError::MissingCodexAuth
            | ModelError::MissingAnthropicApiKey
            | ModelError::Credentials(_)
    )
}

fn model_error_message(error: &ModelError) -> String {
    error.to_string()
}

fn validate_cli(cli: &Cli) -> anyhow::Result<()> {
    if cli.resume.is_some() && matches!(&cli.command, Some(Command::Run { .. })) {
        anyhow::bail!("--resume is only supported for interactive sessions");
    }
    Ok(())
}

fn apply_cli_overrides(cfg: &mut Config, cli: &Cli) -> anyhow::Result<bool> {
    let mut save_config = false;
    if let Some(provider) = &cli.provider {
        cfg.provider = provider.clone();
        if let Some(target) = catalog::login_target_for_provider(provider) {
            cfg.auth = target.auth;
        }
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
    Ok(save_config)
}

fn apply_model_override(cfg: &mut Config, model: &str) -> anyhow::Result<()> {
    let Some((provider, model_name)) = model.split_once('/') else {
        cfg.model = model.to_string();
        return Ok(());
    };
    let provider = provider.trim();
    let model_name = model_name.trim();
    if provider.is_empty() || model_name.is_empty() {
        anyhow::bail!("--model provider/model cannot have an empty provider or model");
    }
    if !catalog::implemented_providers().contains(&provider) {
        anyhow::bail!("unknown provider '{provider}' for --model provider/model");
    }
    cfg.provider = provider.to_string();
    cfg.model = model_name.to_string();
    if let Some(target) = catalog::login_target_for_provider(provider) {
        cfg.auth = target.auth;
    }
    Ok(())
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
    }

    #[test]
    fn cli_anthropic_provider_override_selects_matching_auth() {
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
        assert_eq!(cfg.auth, "anthropic-api-key");
    }

    #[test]
    fn cli_unqualified_model_override_keeps_provider() {
        let mut cfg = Config {
            provider: "openai-codex".into(),
            auth: "codex".into(),
            ..Config::default()
        };
        let cli = Cli {
            provider: None,
            model: Some("custom-model".into()),
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
        assert_eq!(cfg.model, "custom-model");
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
