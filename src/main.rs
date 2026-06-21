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
    let mut cfg = Config::load(cli.config)?;
    if let Some(model) = cli.model {
        cfg.model = model;
    }
    if let Some(cwd) = cli.cwd {
        cfg.cwd = cwd;
    }
    if let Some(max_steps) = cli.max_steps {
        cfg.max_steps = max_steps;
    }
    if let Some(auth) = cli.auth {
        cfg.auth = auth;
    }

    let auth_mode = match cfg.auth.as_str() {
        "codex" => AuthMode::Codex,
        _ => AuthMode::ApiKey,
    };
    let provider = OpenAiProvider::new(cfg.model.clone(), cfg.api_base.clone(), auth_mode)?;
    let registry = tools::registry();
    let ctx = ToolContext {
        cwd: cfg.cwd.clone(),
        max_output_bytes: cfg.max_output_bytes,
    };
    let agent = Agent::new(provider, registry, ctx, cfg.max_steps);

    println!(
        "rho: cwd={} model={} auth={}",
        cfg.cwd.display(),
        cfg.model,
        cfg.auth
    );
    loop {
        print!("rho> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        let prompt = line.trim();
        if prompt.is_empty() {
            continue;
        }
        if prompt == "exit" || prompt == "quit" {
            break;
        }
        match agent.run(prompt.to_string()).await {
            Ok(answer) => println!("{answer}"),
            Err(err) => eprintln!("{err}"),
        }
    }
    Ok(())
}
