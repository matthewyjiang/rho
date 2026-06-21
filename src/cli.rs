use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run one non-interactive automation prompt and print the final answer.
    Run {
        /// Read additional prompt text from stdin.
        #[arg(long)]
        stdin: bool,
        /// Prompt text to send to the agent.
        #[arg(value_name = "PROMPT", num_args = 0..)]
        prompt: Vec<String>,
    },
}
