//! Build argv for `claude -p` subagent runs.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use rho_providers::{model::ReasoningLevelSet, reasoning::ReasoningLevel};

use rho_sdk::{CapabilityKind, PolicyDecision};

use crate::{agent::PromptPolicy, permission::PermissionMode};

/// File name for the materialized system prompt inside a run directory.
pub(crate) const SYSTEM_PROMPT_FILE_NAME: &str = "system-prompt.txt";

/// Claude Code `--permission-mode` values Rho will set on `claude -p`.
///
/// This is not Rho's [`PermissionMode`]. The names collide across products;
/// each variant is a deliberate Claude CLI contract:
/// - [`Self::Plan`] — Rho Plan (investigation / plan scaffolding)
/// - [`Self::BypassPermissions`] — Rho Bypass ("just run", no Claude prompts)
/// - [`Self::DontAsk`] — advisor one-shots, and Auto / Allow edits only when
///   [`dont_ask_preserves_bound_set`] holds; never prompt. Claude still
///   auto-approves read-only Bash and PreToolUse hooks, so this is not a
///   complete `--allowedTools` fence.
///
/// Claude classifier `auto` is intentionally absent because Rho's classifier
/// mode needs its own approval handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaudePermissionMode {
    /// Claude `--permission-mode plan`.
    Plan,
    /// Claude `--permission-mode bypassPermissions`.
    BypassPermissions,
    /// Claude `--permission-mode dontAsk`. Headless runs that must not prompt.
    DontAsk,
}

impl ClaudePermissionMode {
    pub(crate) const fn as_cli_flag(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::BypassPermissions => "bypassPermissions",
            Self::DontAsk => "dontAsk",
        }
    }
}

/// Inputs needed to construct a Claude CLI spawn.
///
/// Model, tools, and inherit config come from the bound runtime contract, not
/// from re-interpreting parent provider/model config.
#[derive(Clone, Debug)]
pub(crate) struct ClaudeSpawnRequest {
    /// Agent system prompt policy. Spawn needs no other definition field, so a
    /// mismatched runtime cannot reach here.
    pub(crate) system_prompt: PromptPolicy,
    /// Claude `--model` value. `None` means omit the flag (Claude inherit).
    pub(crate) model: Option<String>,
    /// Full Claude tool entries from the definition (`Read`, `Bash(git *)`, …).
    pub(crate) tools: Vec<String>,
    pub(crate) inherit_claude_config: bool,
    /// Claude CLI permission mode. Not Rho's permission mode.
    pub(crate) permission_mode: ClaudePermissionMode,
    pub(crate) cwd: PathBuf,
    /// Soft turn cap emitted as `--max-turns`. Claude's flag is undocumented
    /// surface; callers should treat rejection of the flag as a hard error.
    pub(crate) max_turns: u64,
    /// Claude `--effort` value from definition `reasoning:`. `None` omits the flag.
    pub(crate) effort: Option<&'static str>,
    /// Whether the run leaves a resumable Claude session behind.
    pub(crate) session_persistence: SessionPersistence,
    /// Stdin framing. Delegated sessions use stream-json so parents can send
    /// follow-up turns; one-shot keeps plain text.
    pub(crate) input_format: ClaudeInputFormat,
}

/// How the Claude child reads its stdin prompt bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaudeInputFormat {
    /// Default `-p` text prompt. Stdin is closed after one write.
    Text,
    /// NDJSON user turns (`--input-format stream-json`). Stdin stays open for
    /// parent course-corrections until the drain closes it after a result.
    StreamJson,
}

/// Whether a `claude -p` run is worth keeping in Claude's session store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionPersistence {
    /// Keep the session so `claude --resume <id>` works. Delegated agent runs
    /// publish that id in their status file.
    Keep,
    /// Discard it. Rho's own one-shot calls have no resumable identity and
    /// would otherwise fill the user's session list with single-turn runs.
    Discard,
}

/// How the agent system prompt is applied on the Claude CLI.
///
/// Prompt text is never placed on argv. Session code writes a private file in
/// the run directory and passes `--system-prompt-file` /
/// `--append-system-prompt-file` (verified Claude Code flags).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SystemPromptPlan {
    /// Empty `PromptPolicy::Extend`: leave Claude's default system prompt alone.
    Omit,
    /// `PromptPolicy::Replace` body. Parser requires nonempty text.
    Replace(String),
    /// Nonempty `PromptPolicy::Extend` body appended to Claude's default.
    Extend(String),
}

impl SystemPromptPlan {
    pub(crate) fn file_flag(&self) -> Option<&'static str> {
        match self {
            Self::Omit => None,
            Self::Replace(_) => Some("--system-prompt-file"),
            Self::Extend(_) => Some("--append-system-prompt-file"),
        }
    }

    /// Flag that carries the prompt text on argv instead of in a file. Only
    /// safe for Rho's own constant prompts; see [`inline_prompt_args`].
    pub(crate) fn argv_flag(&self) -> Option<&'static str> {
        match self {
            Self::Omit => None,
            Self::Replace(_) => Some("--system-prompt"),
            Self::Extend(_) => Some("--append-system-prompt"),
        }
    }

    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Self::Omit => None,
            Self::Replace(text) | Self::Extend(text) => Some(text.as_str()),
        }
    }
}

/// The full spawn contract: argv is the only carrier of flag decisions, so
/// tests and production read the same values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeSpawnPlan {
    /// Argv without system-prompt flags. Call [`finalize_spawn_args`] to attach
    /// a materialized prompt file path when needed.
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) system_prompt: SystemPromptPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ClaudeSpawnError {
    #[error(
        "claude-cli agents cannot run in Auto or Allow edits unless the child stays \
on its declared tools: Claude dontAsk also auto-approves read-only Bash and \
PreToolUse hooks. Rho therefore refuses when inherit_claude_config is true, \
when a tool uses a specifier (for example Bash(git *)), or when any declared \
tool is not proven to stay inside that Rho approval class. Unknown Claude, \
plugin, and MCP names fail closed. Switch to Plan or Bypass, use only proven \
no-prompt tools, or disable inherited Claude config."
    )]
    DontAskUnbound,
    #[error(
        "claude-cli agents cannot run in Supervised permission mode: \
claude -p cannot prompt interactively for approval. Switch to Auto, Allow edits, Plan, or Bypass, or change the agent. \
Auto and Allow edits only work when every tool is proven not to bypass that \
Rho approval class and inherit_claude_config is false."
    )]
    SupervisedUnsupported,
}

/// Failures while writing the private system-prompt file for a run.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ClaudeSpawnMaterializeError {
    #[error("claude code: could not write system prompt file `{}`: {source}", crate::paths::display(.path))]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Map Rho permission mode onto a Claude CLI permission mode.
///
/// Bypass maps to Claude `bypassPermissions` (no Claude permission layer).
/// Auto and Allow edits map to Claude `dontAsk` only when
/// [`dont_ask_preserves_bound_set`] is true. Claude `dontAsk` also
/// auto-approves built-in read-only Bash and PreToolUse hooks, so a narrowed
/// `Bash(git *)` rule plus `--tools Bash`, a write or process tool, an
/// unknown Claude / plugin / MCP name, or inherited/project hooks, would run
/// actions outside the Rho approval boundary. Advisor one-shots still set
/// [`ClaudePermissionMode::DontAsk`] on the spawn request directly so they
/// stay independent of host permission mode. Supervised is refused because
/// `claude -p` cannot pause for interactive human approval.
pub(crate) fn map_permission_mode(
    mode: PermissionMode,
    tools: &[String],
    inherit_claude_config: bool,
) -> Result<ClaudePermissionMode, ClaudeSpawnError> {
    match mode {
        PermissionMode::Plan => Ok(ClaudePermissionMode::Plan),
        PermissionMode::Bypass => Ok(ClaudePermissionMode::BypassPermissions),
        PermissionMode::Auto | PermissionMode::AllowEdits => {
            if dont_ask_preserves_bound_set(mode, tools, inherit_claude_config) {
                Ok(ClaudePermissionMode::DontAsk)
            } else {
                Err(ClaudeSpawnError::DontAskUnbound)
            }
        }
        PermissionMode::Supervised => Err(ClaudeSpawnError::SupervisedUnsupported),
    }
}

/// Claude `dontAsk` is not limited to `--allowedTools`.
///
/// Anthropic also auto-approves read-only Bash and PreToolUse hooks, and
/// `--allowedTools` itself runs listed tools without prompting. Rho can keep
/// the child on the bound set only when Claude settings (and their hooks)
/// stay unloaded and every declared tool is proven not to bypass this mode's
/// approval class, so `--tools` / `--allowedTools` cannot expose a
/// write/process/unknown tool that skips the remaining Auto / Allow edits
/// gate.
fn dont_ask_preserves_bound_set(
    mode: PermissionMode,
    tools: &[String],
    inherit_claude_config: bool,
) -> bool {
    !inherit_claude_config
        && tools
            .iter()
            .all(|tool| dont_ask_tool_preserves_bound_set(mode, tool))
}

fn dont_ask_tool_preserves_bound_set(mode: PermissionMode, tool: &str) -> bool {
    let base = tool_base_name(tool);
    if base.eq_ignore_ascii_case("Task") {
        // Task is always denied separately.
        return true;
    }
    // Specifiers expose a broader base tool via `--tools`.
    base == tool && dont_ask_bare_tool_preserves_approval_boundary(mode, base)
}

fn dont_ask_bare_tool_preserves_approval_boundary(mode: PermissionMode, base: &str) -> bool {
    // Fail closed: only known Claude built-ins whose Rho class is freely
    // allowed in this mode may ride dontAsk. Write and process tools still
    // need the remaining Auto / Allow edits gate (git-tracked / remembered
    // paths, classifier or human process approval). Unknown, plugin, MCP,
    // and Claude MCP resource tools have no proven class: a server resource
    // URI can do more than Rho Read.
    let Some(kind) = claude_tool_capability_kind(base) else {
        return false;
    };
    matches!(mode.decision_for(kind), PolicyDecision::Allow)
}

/// Maps a Claude built-in base name onto the Rho capability class it can
/// exercise. `None` means unproven (new Claude tools, plugins, MCP, and
/// Claude MCP resource tools whose URI effects are not Rho Read).
fn claude_tool_capability_kind(base: &str) -> Option<CapabilityKind> {
    Some(match base.to_ascii_lowercase().as_str() {
        "read" | "glob" | "grep" | "lsp" => CapabilityKind::Read,
        "webfetch" | "websearch" => CapabilityKind::Network,
        "edit" | "write" | "notebookedit" => CapabilityKind::Write,
        "bash" | "powershell" | "monitor" | "skill" => CapabilityKind::Process,
        _ => return None,
    })
}

/// Identity flags that a frozen workflow argv may keep.
///
/// Permission-sensitive flags (`--permission-mode`, `--setting-sources`,
/// `--tools`, `--allowedTools`, `--disallowedTools`, and any skip-permissions
/// switch) are always taken from the regenerated plan for the effective mode.
const FROZEN_IDENTITY_FLAGS: &[&str] = &["--model", "--effort", "--max-turns"];

/// Overlay frozen identity onto argv generated from the effective bound mode.
///
/// Frozen permission flags cannot widen or replace the mapped Claude mode.
pub(crate) fn apply_frozen_identity_args(
    mut generated: Vec<String>,
    frozen: &[String],
) -> Vec<String> {
    for flag in FROZEN_IDENTITY_FLAGS {
        if let Some(value) = single_flag_value(frozen, flag) {
            set_single_flag_value(&mut generated, flag, value);
        }
    }
    generated
}

fn single_flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
}

fn set_single_flag_value(args: &mut Vec<String>, flag: &str, value: String) {
    if let Some(index) = args.iter().position(|arg| arg == flag) {
        if index + 1 < args.len() {
            args[index + 1] = value;
            return;
        }
    }
    args.push((*flag).to_string());
    args.push(value);
}

/// Map Rho `reasoning:` onto Claude `--effort`.
///
/// Claude accepts `low`, `medium`, `high`, `xhigh`, and `max`. Rho `off` and
/// `minimal` have no Claude counterpart, so they return `None` and callers
/// reject them at parse/bind time. Omit the field entirely to inherit Claude's
/// default effort.
pub(crate) fn claude_effort_flag(level: ReasoningLevel) -> Option<&'static str> {
    match level {
        ReasoningLevel::Off | ReasoningLevel::Minimal => None,
        ReasoningLevel::Low => Some("low"),
        ReasoningLevel::Medium => Some("medium"),
        ReasoningLevel::High => Some("high"),
        ReasoningLevel::Xhigh => Some("xhigh"),
        ReasoningLevel::Max => Some("max"),
    }
}

/// Reasoning levels a Claude run can actually be asked for, derived from
/// [`claude_effort_flag`] so the mapping is stated once.
pub(crate) static CLAUDE_EFFORT_LEVELS: LazyLock<ReasoningLevelSet> = LazyLock::new(|| {
    ReasoningLevelSet::new(
        ReasoningLevel::ALL
            .into_iter()
            .filter(|level| claude_effort_flag(*level).is_some())
            .collect(),
    )
});

/// Build argv for a Claude spawn. Infallible once the request carries a
/// resolved [`ClaudePermissionMode`] (Supervised is refused earlier by
/// [`map_permission_mode`]).
pub(crate) fn build_spawn_plan(request: &ClaudeSpawnRequest) -> ClaudeSpawnPlan {
    let permission_mode = request.permission_mode.as_cli_flag();
    let system_prompt = system_prompt_plan(&request.system_prompt);
    // dontAsk still honors PreToolUse hooks from loaded settings, so those
    // runs pass an explicit empty source list. inherit_claude_config cannot
    // be true on a mapped Auto / Allow edits dontAsk spawn.
    let setting_sources = if matches!(request.permission_mode, ClaudePermissionMode::DontAsk) {
        ""
    } else if request.inherit_claude_config {
        "user,project,local"
    } else {
        "project"
    };

    // `--tools` controls availability from Claude's built-in set (base names).
    // A specifier such as `Bash(git *)` still lists `Bash` here, and any tool
    // that is not proven free for this Rho approval class skips the remaining
    // gate, which is why Auto / Allow edits refuse those shapes under dontAsk.
    // `--allowedTools` carries every declared non-Task entry (bare names and
    // patterns) as separate argv items and executes them without prompting, so
    // unknown names must never reach a mapped Auto / Allow edits spawn. Task
    // is always denied so nested Claude agents stay off.
    let tool_base_names = tool_base_names(&request.tools);
    let allowed_tool_entries = allowed_tool_entries(&request.tools);

    let mut args = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        "--permission-mode".into(),
        permission_mode.into(),
        "--disallowedTools".into(),
        "Task".into(),
        "--setting-sources".into(),
        setting_sources.into(),
        "--strict-mcp-config".into(),
    ];
    if matches!(request.input_format, ClaudeInputFormat::StreamJson) {
        // Keep stdin open for parent course-corrections. The drain encodes the
        // initial prompt and later parent messages as stream-json user turns.
        args.push("--input-format".into());
        args.push("stream-json".into());
    }
    // Bound model is already byte-for-byte; do not resolve aliases here.
    if let Some(model) = &request.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    if let Some(effort) = request.effort {
        args.push("--effort".into());
        args.push(effort.into());
    }
    args.push("--max-turns".into());
    args.push(request.max_turns.to_string());

    match request.session_persistence {
        SessionPersistence::Discard => args.push("--no-session-persistence".into()),
        SessionPersistence::Keep => {}
    }

    // Always set `--tools`, including the explicit empty set, so Claude does
    // not inherit ambient built-in tool availability from user config.
    args.push("--tools".into());
    if tool_base_names.is_empty() {
        args.push(String::new());
    } else {
        // Base names are delimiter-safe (alphanumeric / _ / - only).
        args.push(tool_base_names.join(","));
    }

    if !allowed_tool_entries.is_empty() {
        // Variadic form: one argv element per entry so internal spaces such as
        // `Bash(git *)` round-trip. Commas inside a pattern are rejected at
        // parse time because the CLI also accepts comma-separated lists.
        args.push("--allowedTools".into());
        args.extend(allowed_tool_entries.iter().cloned());
    }

    ClaudeSpawnPlan {
        args,
        cwd: request.cwd.clone(),
        system_prompt,
    }
}

/// Write the system prompt (when present) next to the run status file and return
/// final argv. The prompt file is kept as a run artifact for diagnosis.
///
/// Claude Code accepts `--system-prompt-file` / `--append-system-prompt-file`
/// (verified via `claude --help` / missing-argument responses). Passing a path
/// keeps multiline prompt bytes out of shell/cmd argv while preserving exact
/// Replace vs Extend semantics. User prompt stays on stdin.
///
/// Args stay as [`OsString`] so native paths are not forced through lossy UTF-8
/// conversion. The path token is appended via [`Path::into_os_string`].
pub(crate) fn finalize_spawn_args(
    plan: &ClaudeSpawnPlan,
    output_file: &Path,
) -> Result<Vec<OsString>, ClaudeSpawnMaterializeError> {
    let mut args: Vec<OsString> = plan.args.iter().map(OsString::from).collect();
    let Some(flag) = plan.system_prompt.file_flag() else {
        return Ok(args);
    };
    let text = plan
        .system_prompt
        .text()
        .expect("file flag implies prompt text");
    let path = system_prompt_path(output_file);
    crate::config_writer::write_bytes_atomically(&path, text.as_bytes()).map_err(|source| {
        ClaudeSpawnMaterializeError::Write {
            path: path.clone(),
            source,
        }
    })?;
    args.push(OsString::from(flag));
    // Keep the native OsString path (no to_string_lossy). Exact private bytes
    // already live in the prompt file; argv only carries the path token.
    args.push(path.into_os_string());
    Ok(args)
}

/// Final argv with the system prompt carried on the command line.
///
/// Use only when the prompt is one of Rho's own constants. Argv is readable by
/// other processes, so any prompt built from user or workspace text must go
/// through [`finalize_spawn_args`] and its private file instead.
pub(crate) fn inline_prompt_args(plan: &ClaudeSpawnPlan) -> Vec<OsString> {
    let mut args: Vec<OsString> = plan.args.iter().map(OsString::from).collect();
    let Some(flag) = plan.system_prompt.argv_flag() else {
        return args;
    };
    let text = plan
        .system_prompt
        .text()
        .expect("argv flag implies prompt text");
    args.push(OsString::from(flag));
    args.push(OsString::from(text));
    args
}

fn system_prompt_plan(prompt: &PromptPolicy) -> SystemPromptPlan {
    match prompt {
        PromptPolicy::Replace(text) => SystemPromptPlan::Replace(text.clone()),
        PromptPolicy::Extend(text) if text.is_empty() => SystemPromptPlan::Omit,
        PromptPolicy::Extend(text) => SystemPromptPlan::Extend(text.clone()),
    }
}

/// Base Claude tool names for `--tools` availability.
fn tool_base_names(tools: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for tool in tools {
        let base = tool_base_name(tool);
        if base.eq_ignore_ascii_case("Task") {
            // Task is always denied separately; never make it available.
            continue;
        }
        if !names.iter().any(|existing| existing == base) {
            names.push(base.to_string());
        }
    }
    names
}

/// Every declared non-Task tool entry for `--allowedTools` (bare + patterns).
fn allowed_tool_entries(tools: &[String]) -> Vec<String> {
    tools
        .iter()
        .filter(|tool| !tool_base_name(tool).eq_ignore_ascii_case("Task"))
        .cloned()
        .collect()
}

fn tool_base_name(tool: &str) -> &str {
    tool.split_once('(').map_or(tool, |(base, _)| base)
}

/// True when stderr/stdout looks like Claude rejected `--max-turns`.
pub(crate) fn looks_like_max_turns_unsupported(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("max-turns")
        && (lower.contains("unknown")
            || lower.contains("unexpected")
            || lower.contains("unrecognized")
            || lower.contains("invalid"))
}

pub(crate) fn log_path(output_file: &Path) -> PathBuf {
    output_file.with_file_name(crate::subagent::LOG_FILE_NAME)
}

pub(crate) fn system_prompt_path(output_file: &Path) -> PathBuf {
    output_file.with_file_name(SYSTEM_PROMPT_FILE_NAME)
}

#[cfg(test)]
#[path = "spawn_tests.rs"]
mod tests;
