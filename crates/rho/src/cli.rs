use std::{num::NonZeroUsize, path::PathBuf, time::Duration};

use clap::{Parser, Subcommand, ValueEnum};

use rho_providers::{credentials::CredentialStoreBackend, reasoning::ReasoningLevel};

use crate::{
    app::automation_protocol::parse_duration,
    permission::{PermissionMode, PermissionModeParseError},
};

fn parse_permission_mode(value: &str) -> Result<PermissionMode, PermissionModeParseError> {
    value.parse()
}

fn parse_credential_store_backend(value: &str) -> Result<CredentialStoreBackend, String> {
    CredentialStoreBackend::parse(value).map_err(|error| error.to_string())
}

fn parse_auth_profile(value: &str) -> Result<String, String> {
    let profiles = rho_providers::auth_profiles();
    if profiles.contains(&value) {
        return Ok(value.to_string());
    }
    if rho_providers::provider::is_custom_provider_api_key_auth(value) {
        return Ok(value.to_string());
    }
    Err(format!(
        "invalid value '{value}' for '--auth'; expected one of: {}",
        profiles.join(", ")
    ))
}

/// Output contract used by a non-interactive `rho run` invocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Print only the authoritative final assistant answer.
    #[default]
    Text,
    /// Stream independently versioned JSON Lines events.
    Jsonl,
}

/// Output contract for workflow plans and snapshots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum WorkflowDocumentFormat {
    /// Print a human-readable document.
    #[default]
    Text,
    /// Print one JSON document.
    Json,
}

/// Output contract for workflow execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum WorkflowRunFormat {
    /// Print human-readable state changes.
    Text,
    /// Stream versioned JSON Lines events.
    Jsonl,
}

#[derive(Parser, Debug)]
#[command(name = "rho", version)]
pub struct Cli {
    #[arg(long)]
    pub provider: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long, value_parser = parse_auth_profile)]
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
    /// Override permission mode: bypass, auto, allow_edits, plan, or supervised.
    #[arg(long, value_name = "MODE", value_parser = parse_permission_mode)]
    pub(crate) permission_mode: Option<PermissionMode>,
    /// Persist --provider/--model/--auth/--reasoning overrides to the config file.
    ///
    /// Without this flag, those overrides apply only to the current invocation.
    #[arg(long)]
    pub save: bool,
    /// Resume an existing session by UUID or UUID prefix. Omit the ID to choose from a picker.
    #[arg(short = 'R', long, value_name = "ID", num_args = 0..=1)]
    pub resume: Option<Option<String>>,
    /// Open the interactive TUI with this prompt already submitted.
    ///
    /// This starts a normal session. Use `rho run` when you want one answer and
    /// then exit.
    #[arg(long, value_name = "PROMPT")]
    pub prompt: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run one non-interactive automation prompt and print the final answer.
    Run {
        /// Read additional prompt text from stdin.
        ///
        /// Required when stdin is a pipe or redirected file. Without this flag,
        /// redirected stdin is rejected so prompt text is not silently dropped.
        #[arg(long)]
        stdin: bool,
        /// Write a structured status/result file (JSON) that is updated during
        /// the run and finalized on exit. With `--output text` (the default),
        /// progress and streamed assistant text go to stdout and the run ends
        /// with a completion marker; the result file is the durable final
        /// answer. With `--output jsonl`, stdout stays the JSONL event stream.
        #[arg(long, value_name = "PATH")]
        output_file: Option<PathBuf>,
        /// Select plain final-answer output or a JSON Lines event stream.
        #[arg(long, value_enum, default_value_t)]
        output: OutputFormat,
        /// Override the model-step budget for this run.
        #[arg(long, value_name = "N")]
        max_steps: Option<NonZeroUsize>,
        /// Cancel the run after this wall-clock duration.
        #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
        timeout: Option<Duration>,
        /// Prompt text to send to the agent.
        #[arg(value_name = "PROMPT", num_args = 0..)]
        prompt: Vec<String>,
    },
    /// Watch a delegated agent run in a read-only TUI.
    Attach {
        /// Delegated run ID shown when the agent was started.
        ///
        /// Omit to pick from subagents in this directory.
        #[arg(value_name = "ID")]
        id: Option<String>,
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
    /// Configure or probe provider credential storage.
    CredentialStore {
        #[command(subcommand)]
        command: CredentialStoreCommand,
    },
    /// Update rho using the detected installation method.
    Update,
    /// List, rename, or delete saved sessions.
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
    /// Inspect configured Model Context Protocol servers.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// List, inspect, install, and activate local Agent Plugin packages.
    Plugins {
        #[command(subcommand)]
        command: PluginsCommand,
    },
    /// Validate, plan, run, and inspect deterministic workflows.
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    /// Run local setup diagnostics and print a report.
    ///
    /// Same checks as the interactive `/doctor` overlay. Exits with status 1
    /// when any check fails; warnings exit 0.
    Doctor {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Serve the Agent Client Protocol over stdio for editor/host integration.
    Acp,
    /// Internal supervised workflow planner worker. Not a public command.
    #[command(name = "__workflow_planner_worker", hide = true)]
    WorkflowPlannerWorker,
}

/// Argv entry for the internal supervised planner worker process.
pub const WORKFLOW_PLANNER_WORKER_COMMAND: &str = "__workflow_planner_worker";

#[derive(Subcommand, Debug)]
pub enum WorkflowCommand {
    /// List saved workflow plans and runs for the current workspace.
    List {
        /// Include only saved plans.
        #[arg(long, conflicts_with = "runs")]
        plans: bool,
        /// Include only runs.
        #[arg(long, conflicts_with = "plans")]
        runs: bool,
        /// Limit how many rows to print per section.
        #[arg(long, value_name = "N")]
        limit: Option<NonZeroUsize>,
        /// Print one JSON document instead of text rows.
        #[arg(long)]
        json: bool,
    },
    /// Validate a Starlark workflow without creating durable state.
    Validate {
        /// Workflow entry file under the current workspace.
        #[arg(value_name = "FILE")]
        file: PathBuf,
        /// Supply one non-secret workflow input as KEY=JSON. May be repeated.
        #[arg(long, value_name = "KEY=JSON")]
        input: Vec<String>,
    },
    /// Validate, freeze, and persist an immutable workflow plan.
    Plan {
        /// Workflow entry file under the current workspace.
        #[arg(value_name = "FILE")]
        file: PathBuf,
        /// Supply one non-secret workflow input as KEY=JSON. May be repeated.
        #[arg(long, value_name = "KEY=JSON")]
        input: Vec<String>,
        /// Select human-readable or machine-readable plan output.
        #[arg(long, value_enum, default_value_t)]
        output: WorkflowDocumentFormat,
    },
    /// Create and run a workflow from an immutable plan.
    Run {
        /// Full plan UUID or unique UUID prefix.
        #[arg(value_name = "PLAN_ID")]
        plan_id: String,
        /// Confirm the exact plan digest without an interactive prompt.
        #[arg(long)]
        yes: bool,
        /// Select text or JSON Lines instead of the workflow TUI.
        #[arg(long, value_enum)]
        output: Option<WorkflowRunFormat>,
    },
    /// Read one durable workflow run snapshot.
    Status {
        /// Full run UUID or unique UUID prefix.
        #[arg(value_name = "RUN_ID")]
        run_id: String,
        /// Select human-readable or machine-readable snapshot output.
        #[arg(long, value_enum, default_value_t)]
        output: WorkflowDocumentFormat,
    },
    /// Request cancellation of a durable workflow run.
    Cancel {
        /// Full run UUID or unique UUID prefix.
        #[arg(value_name = "RUN_ID")]
        run_id: String,
    },
    /// Resume a durable workflow run from its frozen graph.
    Resume {
        /// Full run UUID or unique UUID prefix.
        #[arg(value_name = "RUN_ID")]
        run_id: String,
        /// Confirm the frozen graph without an interactive prompt.
        #[arg(long)]
        yes: bool,
        /// Confirm that no prior process remains and relaunch uncertain attempts.
        #[arg(long)]
        recover_uncertain: bool,
        /// Select text or JSON Lines instead of the workflow TUI.
        #[arg(long, value_enum)]
        output: Option<WorkflowRunFormat>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionsCommand {
    /// List saved sessions for the current workspace (or all projects).
    List {
        /// Include sessions from every workspace, not only the current directory.
        #[arg(long)]
        all_projects: bool,
        /// Case-insensitive filter over id, title, and first user message.
        #[arg(long, short = 'q', value_name = "TEXT")]
        search: Option<String>,
        /// Limit how many sessions to print.
        #[arg(long, value_name = "N")]
        limit: Option<NonZeroUsize>,
        /// Print one JSON document instead of text rows.
        #[arg(long)]
        json: bool,
    },
    /// Export a saved session transcript by UUID or UUID prefix.
    Export {
        /// Session UUID or unique prefix.
        #[arg(value_name = "ID")]
        id_prefix: String,
        /// Output path. Omit to write under ~/.rho/exports/.
        #[arg(long, short = 'o', value_name = "PATH")]
        output: Option<PathBuf>,
        /// Explicit format. When omitted, the path extension selects html, md, or json.
        #[arg(long, value_enum)]
        format: Option<crate::export::ExportFormat>,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Delete one or more sessions by UUID or UUID prefix.
    Rm {
        /// Session UUID or unique prefix. May be repeated.
        #[arg(value_name = "ID", required = true, num_args = 1..)]
        ids: Vec<String>,
        /// Delete even when a parent-linked run is still non-terminal.
        ///
        /// Use only for stale Starting/Running artifacts left after a crash.
        #[arg(long)]
        force: bool,
        /// Skip the confirmation prompt for cross-project deletes.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Delete sessions whose workspace directories no longer exist.
    Cleanup {
        /// Delete even when a parent-linked run is still non-terminal.
        ///
        /// Use only for stale Starting/Running artifacts left after a crash.
        #[arg(long)]
        force: bool,
        /// Skip the confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Rename a session by UUID or UUID prefix.
    Rename {
        /// Session UUID or unique prefix.
        #[arg(value_name = "ID")]
        id_prefix: String,
        /// New session title. Multiple words are joined with spaces.
        #[arg(value_name = "TITLE", required = true, num_args = 1.., trailing_var_arg = true)]
        title: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum McpCommand {
    /// List configured MCP servers from the selected config and plugins.
    List {
        /// Print one JSON document instead of text rows.
        #[arg(long)]
        json: bool,
        /// Start enabled servers and report live connection status.
        #[arg(long)]
        connect: bool,
    },
    /// Show one MCP server by identity.
    Show {
        /// Server table key from `[mcp.servers.<id>]`.
        #[arg(value_name = "ID")]
        id: String,
        /// Print one JSON document instead of text.
        #[arg(long)]
        json: bool,
        /// Start enabled servers and report live connection status.
        #[arg(long)]
        connect: bool,
    },
}

/// Target scope for plugin install and link.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum PluginsScope {
    /// User root: `~/.agents/plugins`.
    #[default]
    User,
    /// Project root: `<repository>/.agents/plugins`.
    Project,
}

#[derive(Subcommand, Debug)]
pub enum PluginsCommand {
    /// List discovered Agent Plugin packages.
    List {
        /// Print one JSON document instead of text rows.
        #[arg(long)]
        json: bool,
    },
    /// Inspect one plugin by package name without executing package code.
    Inspect {
        /// Plugin package name from `plugin.json`.
        #[arg(value_name = "NAME")]
        name: String,
        /// Print one JSON document instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Copy a local plugin package into a managed plugins root.
    Install {
        /// Path to a directory that contains `plugin.json`.
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Install under the user or project plugins root.
        #[arg(long, value_enum, default_value_t = PluginsScope::User)]
        scope: PluginsScope,
        /// Replace an existing package at the destination.
        #[arg(long)]
        force: bool,
    },
    /// Symlink a local plugin package into a managed plugins root.
    Link {
        /// Path to a directory that contains `plugin.json`.
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Link under the user or project plugins root.
        #[arg(long, value_enum, default_value_t = PluginsScope::User)]
        scope: PluginsScope,
        /// Replace an existing package at the destination.
        #[arg(long)]
        force: bool,
    },
    /// Enable a discovered plugin for new sessions.
    Enable {
        /// Plugin package name from `plugin.json`.
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Disable a discovered plugin without deleting package files.
    Disable {
        /// Plugin package name from `plugin.json`.
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Remove an installed or linked package from a managed plugins root.
    Remove {
        /// Plugin package name from `plugin.json`.
        #[arg(value_name = "NAME")]
        name: String,
        /// Skip the confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CredentialStoreCommand {
    /// Test a credential backend by writing and deleting a temporary secret.
    Probe {
        /// Backend to test: os or file (`auto` is accepted as an alias for os).
        #[arg(
            value_name = "BACKEND",
            default_value = "os",
            value_parser = parse_credential_store_backend
        )]
        backend: CredentialStoreBackend,
    },
    /// Show the configured credential backend (unset, os, or file).
    Status,
    /// Save the credential backend used by future rho processes.
    Set {
        /// Backend to use: os or file (`auto` is accepted as an alias for os).
        #[arg(value_name = "BACKEND", value_parser = parse_credential_store_backend)]
        backend: CredentialStoreBackend,
    },
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
