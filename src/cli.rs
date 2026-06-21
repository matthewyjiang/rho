use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "rho")]
pub struct Cli {
    #[arg(long)]
    pub provider: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long, value_parser = ["api-key", "codex"])]
    pub auth: Option<String>,
}
