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
    #[arg(long, value_parser = ["api-key", "codex", "anthropic-api-key", "github-copilot", "xai-api-key", "xai-oauth", "moonshot-api-key", "kimi-oauth"])]
    pub auth: Option<String>,
    /// Do not send rho's system prompt, including AGENTS.md and skill context.
    #[arg(long)]
    pub no_system_prompt: bool,
    /// Do not expose any tools to the model.
    #[arg(long)]
    pub no_tools: bool,
    /// Do not expose the delegated-agent tools (agent/agents) to the model.
    #[arg(long, global = true)]
    pub no_subagents: bool,
    /// Select the agent definition used for this session or automation run.
    #[arg(long, global = true, value_name = "ID")]
    pub agent: Option<String>,
    /// Override reasoning level: off, minimal, low, medium, high, xhigh, or max.
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
        /// Stream progress to stdout and write a structured status/result
        /// file (JSON) that is updated during the run and finalized on exit.
        #[arg(long, value_name = "PATH")]
        output_file: Option<PathBuf>,
        /// Prompt text to send to the agent.
        #[arg(value_name = "PROMPT", num_args = 0..)]
        prompt: Vec<String>,
    },
    /// Watch a delegated agent run in a read-only TUI.
    Attach {
        /// Delegated run ID shown when the agent was started.
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Log in to a provider from a browser or device-code flow.
    Login {
        /// Provider to authenticate, for example openai-codex or github-copilot.
        #[arg(value_name = "PROVIDER")]
        provider: String,
        /// Use device-code login instead of opening a local browser callback.
        #[arg(long)]
        device_auth: bool,
    },
    /// Update rho using the detected installation method.
    Update,
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
