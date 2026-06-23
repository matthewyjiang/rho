use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::reasoning::ReasoningLevel;

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
    /// Do not send rho's system prompt, including AGENTS.md and skill context.
    #[arg(long)]
    pub no_system_prompt: bool,
    /// Do not expose any tools to the model.
    #[arg(long)]
    pub no_tools: bool,
    /// Override reasoning level: off, minimal, low, medium, high, or xhigh.
    #[arg(long)]
    pub reasoning: Option<ReasoningLevel>,
    /// Resume an existing session by UUID or UUID prefix. Omit the ID to choose from a picker.
    #[arg(short = 'R', long, value_name = "ID", num_args = 0..=1)]
    pub resume: Option<Option<String>>,
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
