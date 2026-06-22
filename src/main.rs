mod agent;
mod auth;
mod cli;
mod commands;
mod config;
mod credentials;
mod model;
mod prompt;
mod session;
mod skills;
mod tool;
mod tools;
mod transcript;
mod tui;

use std::io::{self, IsTerminal, Read};

use clap::Parser;

use agent::Agent;
use cli::{Cli, Command};
use config::Config;
use model::{build_provider, reasoning_config_value, ModelError, UnavailableProvider};
use session::Session;
use tool::ToolContext;
use tui::TuiInfo;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    validate_cli(&cli)?;
    let config_path = cli.config.clone();
    let mut cfg = Config::load(config_path.clone())?;
    let mut save_config = false;
    if let Some(provider) = cli.provider {
        cfg.provider = provider;
        save_config = true;
    }
    if let Some(model) = cli.model {
        cfg.model = model;
        save_config = true;
    }
    if let Some(auth) = cli.auth {
        cfg.auth = auth;
        save_config = true;
    }
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

    let provider_result = build_provider(
        &cfg.provider,
        &cfg.model,
        reasoning_config_value(&cfg.reasoning_effort),
        reasoning_config_value(&cfg.reasoning_summary),
    );
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
    let registry = tools::registry();
    let cwd = std::env::current_dir()?;
    let ctx = ToolContext {
        cwd: cwd.clone(),
        max_output_bytes: cfg.max_output_bytes,
    };
    let mut agent = Agent::new(provider, registry, ctx);

    match run_prompt {
        Some(prompt) => {
            let answer = agent.run(prompt).await?;
            println!("{answer}");
        }
        None => {
            let session_id = if let Some(id) = &cli.resume {
                let (session, history) = Session::open_by_id(&cwd, id)?;
                let session_id = Some(session.id().to_string());
                agent = agent.with_history(history);
                agent.set_message_sink(move |message| session.append_message(message));
                session_id
            } else {
                None
            };
            let tui_result = tui::run(
                &mut agent,
                TuiInfo {
                    cwd,
                    provider: cfg.provider,
                    model: cfg.model,
                    reasoning_effort: cfg.reasoning_effort,
                    reasoning_summary: cfg.reasoning_summary,
                    auth: cfg.auth,
                    session_id,
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
        ModelError::MissingApiKey | ModelError::MissingCodexAuth | ModelError::Credentials(_)
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
            resume: Some("session-id".into()),
            command: Some(Command::Run {
                stdin: true,
                prompt: Vec::new(),
            }),
        };

        let err = validate_cli(&cli).unwrap_err();

        assert!(err.to_string().contains("--resume is only supported"));
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
