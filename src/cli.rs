use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "rho")]
pub struct Cli {
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    #[arg(long)]
    pub max_steps: Option<usize>,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long, value_parser = ["api-key", "codex"])]
    pub auth: Option<String>,
}
