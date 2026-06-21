mod agent;
mod cli;
mod config;
mod model;
mod prompt;
mod tool;
mod tools;
mod transcript;

use std::io::{self, Write};

use clap::Parser;

use agent::Agent;
use cli::Cli;
use config::Config;
use model::{AuthMode, OpenAiProvider};
use tool::ToolContext;

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

    let auth_mode = match cfg.auth.as_str() {
        "codex" => AuthMode::Codex,
        _ => AuthMode::ApiKey,
    };
    let provider = OpenAiProvider::new(cfg.model.clone(), auth_mode)?;
    let registry = tools::registry();
    let cwd = std::env::current_dir()?;
    let ctx = ToolContext {
        cwd: cwd.clone(),
        max_output_bytes: cfg.max_output_bytes,
    };
    let mut agent = Agent::new(provider, registry, ctx);

    println!(
        "rho: cwd={} provider={} model={} auth={}",
        cwd.display(),
        cfg.provider,
        cfg.model,
        cfg.auth
    );
    loop {
        print!("rho> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            eprintln!("[rho] stopped: stdin closed");
            break;
        }
        let prompt = line.trim();
        if prompt.is_empty() {
            continue;
        }
        if prompt == "exit" || prompt == "quit" {
            eprintln!("[rho] stopped: user requested exit");
            break;
        }
        if prompt == "/reset" {
            agent.reset();
            println!("history reset");
            continue;
        }
        match agent.run(prompt.to_string()).await {
            Ok(answer) => println!("{answer}"),
            Err(err) => eprintln!("[rho] stopped: {err}"),
        }
    }
    Ok(())
}
