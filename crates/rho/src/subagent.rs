//! Durable status and attachment artifacts for delegated agent runs.

use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};

pub const RESULT_FILE_NAME: &str = "result.json";
pub const LOG_FILE_NAME: &str = "log.txt";
pub const ATTACHMENT_FILE_NAME: &str = "events.jsonl";

/// Process-wide ownership for monotonic status read-check-replace.
///
/// Status I/O is already off hot async paths, so one lock is enough to keep a
/// stale Running writer from racing past a terminal Error write. Callers must
/// not re-enter status writers while holding this lock (hooks included).
fn status_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// State machine for a subagent run, persisted in the result file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    #[default]
    Starting,
    Running,
    Ok,
    Error,
    Stopped,
}

impl RunState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ok | Self::Error | Self::Stopped)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Stopped => "stopped",
        }
    }
}

/// Contents of the `--output-file` a subagent writes atomically as it runs.
///
/// The parent process reads this file for status checks and completion
/// detection; the pane or log output is display-only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunStatus {
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_error: Option<String>,
    /// Claude Code session id from a `runtime: claude-cli` run. Resume with
    /// `claude --resume <id>`. Absent for Rho runtime runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    /// Terminal `total_cost_usd` from Claude's result message when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
}

/// Writes the status file atomically (unique temp + replace) so readers never
/// observe a torn write. Repeated updates replace an existing `result.json`.
///
/// Terminal states are sticky on disk: a nonterminal snapshot never replaces an
/// already-terminal status file. This is the shared monotonicity guard used by
/// Claude persistence, executor panic fallback, and other writers so a detached
/// worker cannot overwrite `Error`/`Ok`/`Stopped` with a queued `Running`.
///
/// The existing-status read, terminal check, and atomic replace are serialized
/// under process-wide ownership so concurrent writers cannot interleave a stale
/// nonterminal replace after a terminal write.
pub fn write_status(path: &Path, status: &RunStatus) -> std::io::Result<()> {
    write_status_inner(path, status, /*force*/ false)
}

/// Begin a new run on `path`, deliberately replacing any prior terminal file.
///
/// Use only at run boundaries (executor start, automation reporter start,
/// Claude status sink start). Same-run updates must keep using [`write_status`].
pub fn initialize_status(path: &Path, status: &RunStatus) -> std::io::Result<()> {
    write_status_inner(path, status, /*force*/ true)
}

fn write_status_inner(path: &Path, status: &RunStatus, force: bool) -> std::io::Result<()> {
    let _guard = status_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !force && !status.state.is_terminal() {
        if let Some(existing) = read_status(path) {
            if existing.state.is_terminal() {
                return Ok(());
            }
        }
    }
    #[cfg(test)]
    status_write_hooks::run_after_read(path, status);
    let contents = serde_json::to_vec_pretty(status)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    crate::config_writer::write_bytes_atomically(path, &contents)
}

pub fn read_status(path: &Path) -> Option<RunStatus> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

pub fn directory(id: &str) -> anyhow::Result<PathBuf> {
    validate_id(id)?;
    Ok(crate::paths::rho_dir()?.join("subagents").join(id))
}

fn validate_id(id: &str) -> anyhow::Result<()> {
    if id.len() != 6 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("invalid subagent id '{id}': expected 6 hexadecimal characters");
    }
    Ok(())
}

pub(crate) fn create_private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub(crate) fn secure_directory(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is not a trusted directory", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(crate) fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Test-only hooks for deterministic status-write interleaving.
///
/// Hooks run while the status-write lock is held, so they must not call
/// [`write_status`] / [`initialize_status`] (that would deadlock).
#[cfg(test)]
pub(crate) mod status_write_hooks {
    use super::*;
    use std::sync::Mutex;

    static AFTER_READ: Mutex<Option<Box<dyn Fn(&Path, &RunStatus) + Send>>> = Mutex::new(None);

    pub(crate) fn set_after_read(hook: impl Fn(&Path, &RunStatus) + Send + 'static) {
        *AFTER_READ
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(hook));
    }

    pub(crate) fn clear() {
        *AFTER_READ
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    pub(crate) fn run_after_read(path: &Path, status: &RunStatus) {
        let hook = AFTER_READ
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(hook) = hook.as_ref() {
            hook(path, status);
        }
    }
}

#[cfg(test)]
#[path = "subagent_tests.rs"]
mod tests;
