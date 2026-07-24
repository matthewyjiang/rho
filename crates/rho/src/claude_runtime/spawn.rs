//! Build argv for `claude -p` subagent runs.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{
    agent::{AgentDefinition, AgentRuntime, PromptPolicy},
    permission::PermissionMode,
};

/// File name for the materialized system prompt inside a run directory.
pub(crate) const SYSTEM_PROMPT_FILE_NAME: &str = "system-prompt.txt";

/// Inputs needed to construct a Claude CLI spawn.
///
/// Model, tools, and inherit config come from the bound runtime contract, not
/// from re-interpreting parent provider/model config.
#[derive(Clone, Debug)]
pub(crate) struct ClaudeSpawnRequest {
    pub(crate) definition: AgentDefinition,
    /// Claude `--model` value. `None` means omit the flag (Claude inherit).
    pub(crate) model: Option<String>,
    /// Full Claude tool entries from the definition (`Read`, `Bash(git *)`, …).
    pub(crate) tools: Vec<String>,
    pub(crate) inherit_claude_config: bool,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) cwd: PathBuf,
    /// Soft turn cap emitted as `--max-turns`. Claude's flag is undocumented
    /// surface; callers should treat rejection of the flag as a hard error.
    pub(crate) max_turns: u64,
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

    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Self::Omit => None,
            Self::Replace(text) | Self::Extend(text) => Some(text.as_str()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeSpawnPlan {
    /// Argv without system-prompt flags. Call [`finalize_spawn_args`] to attach
    /// a materialized prompt file path when needed.
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) system_prompt: SystemPromptPlan,
    pub(crate) model: Option<String>,
    pub(crate) permission_mode: String,
    pub(crate) setting_sources: String,
    pub(crate) tool_base_names: Vec<String>,
    /// Full non-Task tool entries for `--allowedTools` (bare names and patterns).
    pub(crate) allowed_tool_entries: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ClaudeSpawnError {
    #[error(
        "claude-cli agents cannot run in Supervised permission mode: \
claude -p cannot prompt for approval. Switch to Plan or Auto, or change the agent."
    )]
    SupervisedUnsupported,
    #[error("internal: Claude spawn requested for non-claude-cli agent")]
    WrongRuntime,
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
    #[error(
        "claude code: system prompt path is not valid UTF-8 (`{}`); refusing lossy argv conversion",
        crate::paths::display(.path)
    )]
    NonUtf8Path { path: PathBuf },
}

/// Map Rho permission mode onto Claude's `--permission-mode`.
///
/// Never maps to `bypassPermissions`. Supervised has no safe non-interactive
/// counterpart, so spawn is refused.
pub(crate) fn map_permission_mode(mode: PermissionMode) -> Result<&'static str, ClaudeSpawnError> {
    match mode {
        PermissionMode::Plan => Ok("plan"),
        PermissionMode::Auto => Ok("dontAsk"),
        PermissionMode::Supervised => Err(ClaudeSpawnError::SupervisedUnsupported),
    }
}

pub(crate) fn build_spawn_plan(
    request: &ClaudeSpawnRequest,
) -> Result<ClaudeSpawnPlan, ClaudeSpawnError> {
    if request.definition.runtime != AgentRuntime::ClaudeCli {
        return Err(ClaudeSpawnError::WrongRuntime);
    }
    let permission_mode = map_permission_mode(request.permission_mode)?.to_string();
    let system_prompt = system_prompt_plan(&request.definition.prompt);
    // Bound model is already byte-for-byte; do not resolve aliases here.
    let model = request.model.clone();
    let setting_sources = if request.inherit_claude_config {
        "user,project,local".to_string()
    } else {
        "project".to_string()
    };

    // `--tools` controls availability from Claude's built-in set (base names).
    // `--allowedTools` carries every declared non-Task entry (bare names and
    // patterns) as separate argv items. Task is always denied so nested Claude
    // agents stay off.
    let tool_base_names = tool_base_names(&request.tools);
    let allowed_tool_entries = allowed_tool_entries(&request.tools);

    let mut args = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        "--permission-mode".into(),
        permission_mode.clone(),
        "--disallowedTools".into(),
        "Task".into(),
        "--setting-sources".into(),
        setting_sources.clone(),
        "--strict-mcp-config".into(),
    ];
    if let Some(model) = &model {
        args.push("--model".into());
        args.push(model.clone());
    }
    args.push("--max-turns".into());
    args.push(request.max_turns.to_string());

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

    Ok(ClaudeSpawnPlan {
        args,
        cwd: request.cwd.clone(),
        system_prompt,
        model,
        permission_mode,
        setting_sources,
        tool_base_names,
        allowed_tool_entries,
    })
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
/// conversion. When the run directory path is not valid UTF-8, materialization
/// fails before writing so the session can surface [`RunState::Error`] clearly.
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
    // Reject before writing so a private prompt file is never left beside a
    // run that cannot spawn with an exact path argv token.
    if path.to_str().is_none() {
        return Err(ClaudeSpawnMaterializeError::NonUtf8Path { path });
    }
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
