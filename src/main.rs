mod agent;
mod cli;
mod config;
mod model;
mod prompt;
mod session;
mod tool;
mod tools;
mod transcript;
mod tui;

use std::io::{self, IsTerminal, Read};

use clap::Parser;

use agent::Agent;
use cli::{Cli, Command};
use config::Config;
use model::{AuthMode, OpenAiProvider};
use session::Session;
use tool::ToolContext;
use tui::TuiInfo;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
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
        cfg.save(config_path)?;
    }

    if cfg.provider != "openai" {
        anyhow::bail!(
            "unsupported provider '{}': only 'openai' is implemented",
            cfg.provider
        );
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
    if cli.resume.is_some() && run_prompt.is_some() {
        anyhow::bail!("--resume is only supported for interactive sessions");
    }

    let auth_mode = match cfg.auth.as_str() {
        "codex" => AuthMode::Codex,
        _ => AuthMode::ApiKey,
    };
    let provider = OpenAiProvider::new_with_reasoning(
        cfg.model.clone(),
        auth_mode,
        reasoning_config_value(&cfg.reasoning_effort),
        reasoning_config_value(&cfg.reasoning_summary),
    )?;
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
            let (session_id, session_path) = if let Some(id) = &cli.resume {
                let (session, history) = Session::open_by_id(&cwd, id)?;
                let session_id = Some(session.id().to_string());
                let session_path = Some(session.path().to_path_buf());
                agent = agent.with_history(history);
                agent.set_session(session);
                (session_id, session_path)
            } else {
                (None, None)
            };
            tui::run(
                &mut agent,
                TuiInfo {
                    cwd,
                    provider: cfg.provider,
                    model: cfg.model,
                    reasoning_effort: cfg.reasoning_effort,
                    reasoning_summary: cfg.reasoning_summary,
                    session_path,
                    session_id,
                },
            )
            .await?;
        }
    }
    Ok(())
}

fn reasoning_config_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(value.to_string())
    }
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
