//! Optional [RTK](https://github.com/rtk-ai/rtk) integration for agent shell
//! commands: `rtk rewrite` turns a command into its token-efficient form, and
//! each run is recorded where `rtk discover` reads Claude Code transcripts.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::tool::ToolResult;
use rho_sdk::ProcessEnvironment;
use serde_json::json;
use tokio::{io::AsyncWriteExt, process::Command, sync::OnceCell};

/// Matches the budget RTK's own agent hooks give `rtk rewrite`. A rewrite
/// measured at ~30 ms with rtk 1.1.0; a hung binary must not stall the call.
const REWRITE_TIMEOUT: Duration = Duration::from_secs(2);
static DISCOVER_LOG_NAME: LazyLock<String> =
    LazyLock::new(|| format!("rho-{}-{}.jsonl", std::process::id(), uuid::Uuid::new_v4()));

/// Whether an `rtk` 0.23+ binary is on `PATH`. Blocks on `rtk --version`.
///
/// # Next major
///
/// NEXT_MAJOR(rho-agent-tools): make the `rtk` module private and remove `is_available` and `rewrite`; enable RTK only through `ShellToolOptions::rtk`.
///
/// They stay public for semver and serve the legacy `Bash`/`PowerShell` `rtk_enabled` path.
pub fn is_available() -> bool {
    let Ok(output) = std::process::Command::new("rtk").arg("--version").output() else {
        return false;
    };
    output.status.success() && supports_rewrite(&String::from_utf8_lossy(&output.stdout))
}

fn supports_rewrite(version: &str) -> bool {
    let Some((major, minor, _patch)) = parse_version(version) else {
        return true;
    };
    major > 0 || minor >= 23
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let version = version
        .trim()
        .strip_prefix("rtk ")
        .unwrap_or(version.trim());
    let mut parts = version.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.split_whitespace().next()?.parse().ok()?,
    ))
}

/// The `rtk rewrite` form of `command`, or `None` to run it unchanged.
///
/// # Next major
///
/// NEXT_MAJOR(rho-agent-tools): make the `rtk` module private and remove `is_available` and `rewrite`; enable RTK only through `ShellToolOptions::rtk`.
pub async fn rewrite(command: &str) -> Option<String> {
    if disabled_by_env() {
        return None;
    }
    rewrite_with(&mut Command::new("rtk"), command).await
}

/// `rtk rewrite` ignores a process-level `RTK_DISABLED`, so Rho honors it.
fn disabled_by_env() -> bool {
    std::env::var("RTK_DISABLED").ok().as_deref() == Some("1")
}

async fn rewrite_with(rtk: &mut Command, command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() || command.starts_with("rtk ") {
        return None;
    }

    let output = tokio::time::timeout(
        REWRITE_TIMEOUT,
        rtk.arg("rewrite").arg(command).kill_on_drop(true).output(),
    )
    .await
    .ok()?
    .ok()?;

    // 0 and 3 print a rewrite (3 only asks the host to prompt, and Rho's own
    // policy authorizes the rewritten command); 1 and 2 leave it unchanged.
    let code = output.status.code()?;
    if code != 0 && code != 3 {
        return None;
    }

    let rewritten = String::from_utf8(output.stdout).ok()?;
    let rewritten = rewritten.trim();
    (!rewritten.is_empty() && rewritten != command).then(|| rewritten.to_string())
}

/// RTK as used by the SDK shell tool: rewrites commands before they are
/// authorized and records each run for `rtk discover`.
///
/// Availability (`rtk --version` at 0.23 or newer) is probed once, on the first
/// command, so building a tool set never blocks on spawning `rtk`. Nothing is
/// rewritten or recorded while RTK is unavailable.
#[derive(Clone, Debug)]
pub(crate) struct Rtk {
    program: OsString,
    /// Leading arguments, e.g. a fake-rtk script run through `sh` in tests.
    program_args: Vec<OsString>,
    projects_dir: Option<PathBuf>,
    available: Arc<OnceCell<bool>>,
}

/// A command ready to authorize, and its pending discover record.
pub(crate) struct Prepared {
    pub(crate) command: String,
    pub(crate) record: Option<DiscoverRecord>,
}

impl Prepared {
    pub(crate) fn unchanged(command: String) -> Self {
        Self {
            command,
            record: None,
        }
    }
}

/// One `rtk discover` record, written after the command it names finishes.
pub(crate) struct DiscoverRecord {
    projects_dir: PathBuf,
    command: String,
}

impl DiscoverRecord {
    /// Best effort: analytics never fail the command.
    pub(crate) async fn write(self, cwd: &Path, run: CommandRun) {
        let _ = log_execution_in_projects_dir(&self.projects_dir, cwd, &self.command, run).await;
    }
}

impl Rtk {
    /// `rtk` from `PATH`, logging under the Claude Code projects directory.
    /// `None` when `RTK_DISABLED=1` is set.
    pub(crate) fn system() -> Option<Self> {
        (!disabled_by_env()).then(|| Self::new("rtk".into(), Vec::new(), discover_projects_dir()))
    }

    pub(crate) fn new(
        program: OsString,
        program_args: Vec<OsString>,
        projects_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            program,
            program_args,
            projects_dir,
            available: Arc::new(OnceCell::new()),
        }
    }

    /// Rewrites `command` when RTK has a better form, and plans its record.
    /// `rtk` runs in the command's directory with its environment, so it
    /// reads the same project config and never sees scrubbed credentials.
    pub(crate) async fn prepare(
        &self,
        command: String,
        cwd: &Path,
        environment: &ProcessEnvironment,
    ) -> Prepared {
        let Some(mut rtk) = self.command(cwd, environment) else {
            return Prepared::unchanged(command);
        };
        if !self.is_available(cwd, environment).await {
            return Prepared::unchanged(command);
        }
        let command = rewrite_with(&mut rtk, &command).await.unwrap_or(command);
        let record = self
            .projects_dir
            .clone()
            .map(|projects_dir| DiscoverRecord {
                projects_dir,
                command: command.clone(),
            });
        Prepared { command, record }
    }

    /// `None` when `environment` cannot be applied; RTK then stays out of the way.
    fn command(&self, cwd: &Path, environment: &ProcessEnvironment) -> Option<Command> {
        let mut command = Command::new(&self.program);
        command.args(&self.program_args).current_dir(cwd);
        crate::apply_process_environment(&mut command, environment).ok()?;
        Some(command)
    }

    async fn is_available(&self, cwd: &Path, environment: &ProcessEnvironment) -> bool {
        *self
            .available
            .get_or_init(|| async {
                let Some(mut command) = self.command(cwd, environment) else {
                    return false;
                };
                let version = tokio::time::timeout(
                    REWRITE_TIMEOUT,
                    command.arg("--version").kill_on_drop(true).output(),
                )
                .await;
                matches!(version, Ok(Ok(output)) if output.status.success()
                    && supports_rewrite(&String::from_utf8_lossy(&output.stdout)))
            })
            .await
    }
}

/// What `rtk discover` needs from a finished command: output size, not text.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommandRun {
    pub(crate) output_len: usize,
    pub(crate) failed: bool,
}

impl From<&ToolResult> for CommandRun {
    fn from(result: &ToolResult) -> Self {
        Self {
            output_len: result.content.len(),
            failed: !result.ok,
        }
    }
}

pub(super) async fn log_execution(cwd: &Path, command: &str, result: &ToolResult) {
    let Some(projects_dir) = discover_projects_dir() else {
        return;
    };
    let _ = log_execution_in_projects_dir(&projects_dir, cwd, command, result.into()).await;
}

fn discover_projects_dir() -> Option<PathBuf> {
    let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::paths::home_dir().map(|home| home.join(".claude")))?;
    Some(claude_dir.join("projects"))
}

async fn log_execution_in_projects_dir(
    projects_dir: &Path,
    cwd: &Path,
    command: &str,
    run: CommandRun,
) -> std::io::Result<()> {
    let project_dir = projects_dir
        .join(encode_project_path(cwd))
        .join("rho-sessions");
    tokio::fs::create_dir_all(&project_dir).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&project_dir, std::fs::Permissions::from_mode(0o700)).await?;
    }

    let path = project_dir.join(DISCOVER_LOG_NAME.as_str());
    let tool_use_id = format!("rho-{}", uuid::Uuid::new_v4());
    let assistant = json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": tool_use_id,
                "name": "Bash",
                "input": { "command": command }
            }]
        }
    });
    let output_size = " ".repeat(run.output_len);
    let result_entry = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": output_size,
                "is_error": run.failed
            }]
        }
    });
    let mut entry = serde_json::to_vec(&assistant).map_err(std::io::Error::other)?;
    entry.push(b'\n');
    entry.extend(serde_json::to_vec(&result_entry).map_err(std::io::Error::other)?);
    entry.push(b'\n');

    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    file.write_all(&entry).await?;
    file.flush().await
}

fn encode_project_path(path: &Path) -> String {
    const SANITIZED_CHARS: &[char] = &['/', '.', '_', '\\', ':', ' ', '[', ']'];

    path.to_string_lossy()
        .chars()
        .map(|ch| {
            if !ch.is_ascii() || SANITIZED_CHARS.contains(&ch) {
                '-'
            } else {
                ch
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "rtk_tests.rs"]
mod tests;
