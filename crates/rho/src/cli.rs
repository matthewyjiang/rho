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

/// The shortest `rho daemon run --unload-idle-after`. Shorter, a session
/// could unload between one client's consecutive requests, as a reopen
/// takes up to 41 ms measured (`UNLOAD_IDLE_AFTER` has the receipt), and
/// the sweep, which runs once per period, would spin.
const MIN_UNLOAD_IDLE_AFTER: Duration = Duration::from_millis(100);

fn parse_unload_idle_after(value: &str) -> Result<Duration, String> {
    let duration = parse_duration(value)?;
    if duration < MIN_UNLOAD_IDLE_AFTER {
        return Err(format!(
            "invalid duration '{value}': must be at least {}",
            humantime::format_duration(MIN_UNLOAD_IDLE_AFTER)
        ));
    }
    Ok(duration)
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

/// How `rho attach` writes a session's events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum AttachOutput {
    /// Stream the conversation to stdout, and notices to stderr.
    #[default]
    Text,
    /// Stream `rho run --output jsonl`'s JSON Lines events.
    Jsonl,
}

/// Where text-mode `rho attach` reads the lines that answer and steer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum AttachInput {
    /// Stdin, when it is the terminal and `rho attach` runs in its
    /// foreground; otherwise requests are only told.
    #[default]
    Terminal,
    /// Stdin, whatever it is, such as a pipe a script writes answers to.
    Stdin,
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

#[derive(Parser, Clone, Debug)]
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
    /// Add a directory to the workspace scope. Repeat for more directories.
    ///
    /// Checked permission modes treat reads there like workspace reads, and
    /// its AGENTS.md files load with the project's. Interactive sessions save
    /// added directories and restore them on resume.
    #[arg(long = "add-dir", value_name = "PATH", global = true)]
    pub add_dirs: Vec<PathBuf>,
    /// Persist --provider/--model/--auth/--reasoning overrides to the config file.
    ///
    /// Without this flag, those overrides apply only to the current invocation.
    #[arg(long)]
    pub save: bool,
    /// Keep this interactive session in memory without saving its transcript or prompt history.
    #[arg(long, conflicts_with_all = ["resume", "continue_latest"])]
    pub no_save: bool,
    /// Resume an existing session by UUID or UUID prefix. Omit the ID to choose from a picker.
    #[arg(short = 'R', long, value_name = "ID", num_args = 0..=1)]
    pub resume: Option<Option<String>>,
    /// Resume the most recent session in the current workspace without a picker.
    ///
    /// Starts a new session when the workspace has no saved sessions yet.
    #[arg(short = 'c', long = "continue", conflicts_with = "resume")]
    pub continue_latest: bool,
    /// Open the interactive TUI with this prompt already submitted.
    ///
    /// This starts a normal session. Use `rho run` when you want one answer and
    /// then exit.
    #[arg(long, value_name = "PROMPT")]
    pub prompt: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Clone, Debug)]
pub enum ModelPromptCommand {
    /// Open custom system instructions for a model in $VISUAL or $EDITOR.
    #[command(after_help = "Examples:
  rho model-prompt edit
      Choose a model from a searchable picker, then edit its instructions.
  rho model-prompt edit --provider openai-codex
      Choose from locally known models for one provider.
  rho model-prompt edit --provider openai-codex --model gpt-6-astra
      Edit instructions for an exact provider/model pair.
  rho model-prompt edit --model @local
      Resolve a model alias from your config.
  VISUAL='code --wait' rho model-prompt edit
      Use an editor that waits until you close the file.
  rho --config /path/to/config.toml model-prompt edit
      Use a different configuration file.

Editing:
  Without --model, opens a picker. Type to filter, Enter to edit, Esc to cancel.
  The picker uses local catalogs without requiring login or fetching models.
  Non-interactive callers must pass --model explicitly.
  Uses $VISUAL, falling back to $EDITOR. Set one before running this command.
  Opens a private draft of the matching file in ~/.rho/model-prompts/.
  If none exists, starts a draft with provider, model, and mode frontmatter.
  Keep provider/model unchanged and write Markdown instructions below the
  closing --- line. Save and exit the editor to validate and install the file.
  Leaving the draft unchanged cancels. Invalid edits preserve a recovery draft.

Modes:
  mode: append   Add to Rho's default behavioral instructions. This is the default.
  mode: replace  Replace only that behavioral block, not tool instructions,
                 AGENTS.md, skills, or permission rules.

Scope:
  Instructions are global for the exact provider/model pair, not project-local.
  Editing works offline and does not start a session or change your default model.
  Sessions load changes at startup, model switch, /new, or resume, not each turn.")]
    Edit {
        /// Filter the picker by provider; with --model, override the configured provider.
        #[arg(long)]
        provider: Option<String>,
        /// Exact model ID or @alias. Omit to choose from an interactive picker.
        #[arg(long)]
        model: Option<String>,
    },
}

#[derive(Subcommand, Clone, Debug)]
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
        /// Do not save the run's session or its Rho and Claude subagents'
        /// sessions, so none is left to export or resume.
        ///
        /// Cursor and Antigravity subagents keep their own CLI sessions, and
        /// workflow nodes the run starts save theirs as part of the workflow
        /// run's record.
        #[arg(long)]
        no_save: bool,
        /// Start the run in the rho daemon, starting one if none runs, and
        /// exit once its prompt is saved, printing the session's id. The
        /// turn runs on; `rho attach <ID>` follows it and answers its
        /// approvals and questions. `--timeout` is enforced by the daemon.
        #[arg(long, conflicts_with_all = ["output_file", "output", "max_steps", "no_save"])]
        detach: bool,
        /// Prompt text to send to the agent.
        #[arg(value_name = "PROMPT", num_args = 0..)]
        prompt: Vec<String>,
    },
    /// Follow a daemon session's turn until it ends, and exit as it ended.
    ///
    /// Without a turn under way, report the session's last turn. In text
    /// mode, lines typed at the terminal answer the turn's requests, steer
    /// it, or cancel it with `/cancel`. Ctrl+C detaches; the turn runs on in
    /// the daemon.
    Attach {
        /// The session's id, or its job's.
        #[arg(value_name = "ID")]
        id: String,
        /// Write the session's events as text or as the JSON Lines stream
        /// `rho run --output jsonl` writes.
        #[arg(long, value_enum, default_value_t)]
        output: AttachOutput,
        /// In text mode, where to read the lines that answer the turn's
        /// requests and steer it.
        #[arg(long, value_enum, default_value_t)]
        input: AttachInput,
    },
    /// Log in to a provider with an API key, browser, or device-code flow.
    Login {
        /// Provider to authenticate, for example openai-codex or github-copilot,
        /// or `antigravity` to run agy's own sign-in for Antigravity subagents.
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
    /// Customize system instructions for an exact provider/model pair.
    #[command(after_help = "Quick start:
  rho model-prompt edit
  rho model-prompt edit --provider openai-codex --model gpt-6-astra
  rho model-prompt edit --model @local

Uses $VISUAL, falling back to $EDITOR, to edit Markdown instructions stored in
~/.rho/model-prompts/. With no --model, opens a searchable model picker.
Use --provider to narrow the list, or --model to skip the picker.
Non-interactive callers must pass --model explicitly.
New files append to Rho's default behavioral instructions; mode: replace replaces
only that behavioral block. Instructions apply globally, not just to this project.
This edits instructions offline; it does not send a prompt or start a session.

Run rho model-prompt edit --help for editor setup, saving, and reload behavior.")]
    ModelPrompt {
        #[command(subcommand)]
        command: ModelPromptCommand,
    },
    /// Update rho using the detected installation method.
    Update,
    /// Uninstall Rho after confirmation, preserving user data by default.
    Uninstall {
        /// Also permanently delete local Rho user data under ~/.rho.
        #[arg(long)]
        purge: bool,
        /// Preview removal paths without changing anything or prompting.
        #[arg(long)]
        dry_run: bool,
    },
    /// List, rename, or delete saved sessions.
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
    /// Inspect configured Model Context Protocol servers, or serve rho's
    /// sessions as one.
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
    Acp(AcpArgs),
    /// Run or control the rho daemon for this rho home.
    ///
    /// Without a subcommand, runs the daemon in the foreground.
    Daemon {
        #[command(subcommand)]
        command: Option<DaemonCommand>,
    },
    /// Internal supervised workflow planner worker. Not a public command.
    #[command(name = "__workflow_planner_worker", hide = true)]
    WorkflowPlannerWorker,
    /// Debug-only scripted ACP agent on stdio for offline external-runtime
    /// E2E tests. Refuses to run outside `RHO_TUI_TEST_MODE=matrix`.
    #[cfg(debug_assertions)]
    #[command(name = "__acp-fixture-agent", hide = true)]
    AcpFixtureAgent(AcpFixtureAgentArgs),
}

/// Options for `rho acp`.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct AcpArgs {
    /// Mark the host as T3 Code: subagents run as T3 child threads through
    /// its `t3-code` MCP server. Wake-ups for background work are not yet
    /// available with the rho daemon (#1670).
    #[arg(long)]
    pub t3code: bool,
    /// Run terminal authentication for an auth profile instead of serving ACP.
    /// Hosts append this option to the configured agent invocation.
    #[arg(long, value_name = "AUTH")]
    pub login: Option<String>,
}

/// Inputs for the debug-only scripted ACP agent.
#[cfg(debug_assertions)]
#[derive(clap::Args, Clone, Debug)]
pub struct AcpFixtureAgentArgs {
    /// Script JSON: handshake outcomes, session modes, and per-turn steps.
    #[arg(long, value_name = "PATH")]
    pub script: PathBuf,
    /// Append every request Rho sends and every reply it gives here (JSONL).
    #[arg(long, value_name = "PATH")]
    pub journal: PathBuf,
}

/// Argv entry for the internal supervised planner worker process.
pub const WORKFLOW_PLANNER_WORKER_COMMAND: &str = "__workflow_planner_worker";

#[derive(Subcommand, Clone, Debug)]
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
        /// Confirm that no prior process remains and recover uncertain attempts.
        /// A Rho agent attempt continues its saved session; others restart.
        #[arg(long)]
        recover_uncertain: bool,
        /// Report how each uncertain attempt would recover without changing the run.
        #[arg(long)]
        dry_run: bool,
        /// Select text or JSON Lines instead of the workflow TUI.
        #[arg(long, value_enum)]
        output: Option<WorkflowRunFormat>,
    },
}

#[derive(Subcommand, Clone, Debug, PartialEq, Eq)]
pub enum DaemonCommand {
    /// Run the daemon in the foreground until stopped. The default.
    Run {
        /// Unload a session once it has sat idle this long: no turn under
        /// way and no client attached. Unloading closes its MCP servers and
        /// lets other rho processes open it; the next prompt or attach
        /// reopens it. A session that saved nothing is discarded instead.
        /// Defaults to 60s; at least 100ms.
        #[arg(long, value_name = "DURATION", value_parser = parse_unload_idle_after)]
        unload_idle_after: Option<Duration>,
        /// The lock and socket descriptors a daemon that re-executes itself
        /// in place hands the new binary. Only the daemon passes it.
        #[arg(long, value_name = "LOCK:SOCKET", hide = true)]
        exec_handoff: Option<String>,
    },
    /// Start the daemon in the background unless it is already running,
    /// and wait until it answers.
    ///
    /// Exits 0 once a daemon runs, including one that already did. Waits
    /// for a daemon that is stopping to exit before starting a new one.
    Start,
    /// Show the running daemon's pid, version, uptime, socket, and any
    /// upgrade it waits to run.
    ///
    /// Never starts a daemon. Exits with status 1 when none is running.
    Status {
        /// Print one JSON document instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Stop the running daemon and wait for it to exit.
    ///
    /// Exits with status 1 when none is running.
    Stop,
    /// Restart the running daemon in place and wait until it answers again.
    ///
    /// The daemon keeps its pid, socket, and lock, so clients reconnect to
    /// it rather than start another, and re-executes its executable, which
    /// runs an installed upgrade at once. Turns under way are interrupted
    /// and recorded as interrupted. Starts the daemon when none is running.
    Restart,
}

#[derive(Subcommand, Clone, Debug)]
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

#[derive(Subcommand, Clone, Debug)]
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
    /// Serve the rho daemon's sessions as an MCP server: list, read, start,
    /// prompt, steer, cancel, and wait for them.
    ///
    /// Over stdio by default, for a host that starts it as a command. With
    /// `--http`, over streamable HTTP on a loopback address, for hosts that
    /// reach MCP servers by URL through a tunnel; every request then needs
    /// a bearer token from `rho mcp token rotate`. No tool answers a
    /// session's approvals or questions.
    Serve(McpServeArgs),
    /// Manage the bearer tokens `rho mcp serve --http` accepts.
    Token {
        #[command(subcommand)]
        command: McpTokenCommand,
    },
}

#[derive(clap::Args, Clone, Debug)]
pub struct McpServeArgs {
    /// Serve streamable HTTP on this loopback address, such as
    /// 127.0.0.1:8765, instead of stdio.
    #[arg(long, value_name = "ADDR")]
    pub http: Option<std::net::SocketAddr>,
    /// With `--http`, also accept this `Host` header, such as the tunnel's
    /// public hostname. May be repeated.
    #[arg(long, value_name = "HOST", requires = "http")]
    pub allow_host: Vec<String>,
    /// With `--http`, accept requests from this browser origin, such as
    /// https://app.example.com. Any other `Origin` is refused. May be
    /// repeated.
    #[arg(long, value_name = "ORIGIN", requires = "http")]
    pub allow_origin: Vec<String>,
    /// The deepest a session this server starts or prompts may run: 1 for
    /// a session started by a person's or an outside agent's call, one more
    /// for each session in between. Defaults to 3.
    #[arg(long, value_name = "N")]
    pub max_depth: Option<u32>,
    /// How many sessions each caller (each token, over HTTP) may have
    /// running at once. Defaults to 10.
    #[arg(long, value_name = "N")]
    pub max_sessions: Option<u32>,
    /// Over stdio, what the host may do: `read` lists, reads, and waits
    /// for sessions and refuses every other tool; `write`, the default,
    /// also starts, prompts, steers, and cancels them. With `--http` each
    /// token carries its own scope instead.
    #[arg(long, value_enum, conflicts_with = "http")]
    pub scope: Option<McpTokenScope>,
}

#[derive(Subcommand, Clone, Debug)]
pub enum McpTokenCommand {
    /// Create token NAME, or give it a new secret, and print the secret
    /// once. The old secret stops working at once.
    Rotate {
        /// The token's name, which refusals and `list` show.
        #[arg(value_name = "NAME")]
        name: String,
        /// What the token may do: `read` lists, reads, and waits for
        /// sessions; `write` also starts, prompts, steers, and cancels
        /// them. A new token defaults to `read`; rotating keeps the
        /// token's scope unless given.
        #[arg(long, value_enum)]
        scope: Option<McpTokenScope>,
        /// A directory whose sessions, and those of everything under it,
        /// the token may reach. May be repeated; a new token needs at least
        /// one, and rotating keeps the token's list unless given.
        #[arg(long, value_name = "DIR")]
        workspace: Vec<PathBuf>,
    },
    /// List the tokens: their names, scopes, and workspaces.
    List {
        /// Print one JSON document instead of text rows.
        #[arg(long)]
        json: bool,
    },
    /// Delete token NAME; requests with it are refused at once.
    Revoke {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

/// What a caller of the MCP server may do: an HTTP token, or the host of a
/// stdio server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum McpTokenScope {
    Read,
    Write,
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

#[derive(Subcommand, Clone, Debug)]
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
    /// Copy a plugin package from a local path or git repository into a managed plugins root.
    Install {
        /// Local directory with `plugin.json`, a GitHub link
        /// (`https://github.com/<owner>/<repo>[/tree/<ref>/<path>]`), or a git URL.
        #[arg(value_name = "SOURCE")]
        source: std::ffi::OsString,
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
    /// Refetch git-installed plugins and replace them when their ref moved.
    Update {
        /// Plugin package name; omit to update every git-installed plugin.
        #[arg(value_name = "NAME")]
        name: Option<String>,
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

#[derive(Subcommand, Clone, Debug)]
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
