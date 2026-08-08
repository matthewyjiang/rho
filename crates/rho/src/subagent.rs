//! Durable status and attachment artifacts for delegated agent runs.

use std::{
    fs::{File, OpenOptions},
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::agent::AgentRuntime;

mod storage;
pub(crate) use storage::{
    is_trusted_directory, lock_parent_for_cleanup, release_run_directory, reserve_run_directory,
    resolve_run_directory, RunPlacement,
};

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
    /// Backend that executes this run (`rho` or `claude-cli`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<AgentRuntime>,
    /// Unix seconds when the Starting boundary was first written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    /// Unix seconds when the run first entered a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    #[serde(default)]
    pub turns: u64,
    /// Cumulative input tokens when known. Absent means unknown (for example a
    /// cancelled Claude run that never emitted a terminal usage payload).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Cumulative output tokens when known. Absent means unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
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
    /// Parent interactive session that spawned this run, when known.
    ///
    /// Used for cascade cleanup when that session is deleted. Absent on older
    /// result files and on top-level automation runs with no parent session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
}

impl RunStatus {
    /// Elapsed wall time from [`Self::started_at`] to finish or `now_unix_secs`.
    pub fn elapsed_duration(&self, now_unix_secs: u64) -> Option<Duration> {
        let started = self.started_at?;
        let end = self.finished_at.unwrap_or(now_unix_secs).max(started);
        Some(Duration::from_secs(end - started))
    }

    /// Stamp [`Self::finished_at`] once when entering a terminal state.
    pub fn mark_finished_now(&mut self) {
        if self.state.is_terminal() && self.finished_at.is_none() {
            self.finished_at = Some(unix_now_secs());
        }
    }
}

/// Current Unix time in whole seconds.
pub fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Compact elapsed label for rails and attach chrome (`12s`, `1m 05s`, `2h 03m`).
pub fn format_elapsed_secs(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}h {minutes:02}m")
}

/// Convert a provider-reported USD amount into microdollars for session totals.
pub fn usd_to_micros(usd: f64) -> u64 {
    if !usd.is_finite() || usd <= 0.0 {
        return 0;
    }
    let micros = (usd * 1_000_000.0).round();
    if micros >= u64::MAX as f64 {
        u64::MAX
    } else {
        micros as u64
    }
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
/// under process-wide ownership so concurrent writers in the same process cannot
/// interleave a stale nonterminal replace after a terminal write. This
/// monotonicity is single-process only: concurrent `rho` processes can still
/// demote a terminal status if they write the same path.
///
/// Nonterminal snapshots are written without an `fsync`; terminal states are
/// flushed. A crash can therefore lose the last in-progress snapshot but never
/// the recorded outcome.
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
    // One read covers monotonicity and finish-time preservation for same-run updates.
    let existing = if force { None } else { read_status(path) };
    if !force
        && !status.state.is_terminal()
        && existing
            .as_ref()
            .is_some_and(|existing| existing.state.is_terminal())
    {
        return Ok(());
    }
    #[cfg(test)]
    status_write_hooks::run_after_read(path, status);
    // Durable finish time for attach elapsed, even when a caller forgot to stamp.
    let mut status = status.clone();
    if status.state.is_terminal() && status.finished_at.is_none() {
        // Same-run terminal upgrades (Error -> Stopped, etc.) keep the first finish.
        let preserved = (!force)
            .then_some(existing.as_ref())
            .flatten()
            .filter(|existing| existing.state.is_terminal())
            .and_then(|existing| existing.finished_at);
        if let Some(finished_at) = preserved {
            status.finished_at = Some(finished_at);
        }
        status.mark_finished_now();
    }
    let contents = serde_json::to_vec_pretty(&status)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    // In-progress snapshots are rewritten every couple of seconds and readers
    // only poll the newest one, so they do not earn an `fsync`: paying one per
    // update caps the status writer at a few hundred writes per second and
    // starves the attachment journal behind it. Run boundaries and terminal
    // states are the states a later `rho attach` must still find after a crash.
    let durability = if force || status.state.is_terminal() {
        crate::config_writer::WriteDurability::Durable
    } else {
        crate::config_writer::WriteDurability::Replaceable
    };
    crate::config_writer::write_bytes_atomically_with_durability(path, &contents, durability)
}

pub fn read_status(path: &Path) -> Option<RunStatus> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Validate a 6-char hex run id and return its canonical lowercase form.
///
/// Creation always uses lowercase paths. Accepting mixed case and normalizing
/// keeps `rho attach` portable across case-insensitive (macOS default) and
/// case-sensitive (typical Linux) filesystems.
pub fn normalize_id(id: &str) -> anyhow::Result<String> {
    if id.len() != 6 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("invalid subagent id '{id}': expected 6 hexadecimal characters");
    }
    Ok(id.to_ascii_lowercase())
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

    type AfterReadHook = Box<dyn Fn(&Path, &RunStatus) + Send>;

    static AFTER_READ: Mutex<Option<AfterReadHook>> = Mutex::new(None);

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
