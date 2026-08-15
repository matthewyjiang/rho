//! Persist Claude Code rate-limit windows for `/limits`.
//!
//! This is not a credential path. Rho never stores Claude tokens; it only
//! remembers what prior streams reported.
//!
//! Each stream event is one window (`five_hour`, `seven_day`, …). State keeps
//! the newest observation per window so `/limits` can show every known bucket.
//! Per-window ordering is unix-epoch nanoseconds plus a per-process nonce and
//! monotonic sequence so concurrent Rho processes and equal timestamps cannot
//! let an older observation win. Compare-and-replace runs under a cross-process
//! lock.

use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use rho_providers::file_lock::FileLock;

use super::stream::RateLimitInfo;

const STATE_FILE_NAME: &str = "rate-limits.json";
/// Pre-cache top-level file; still read once so existing observations survive.
const LEGACY_STATE_FILE_NAME: &str = "claude-rate-limit.json";
const LOCK_FILE_SUFFIX: &str = ".lock";
const LOCK_RETRY_LIMIT: u32 = 1_000;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);

/// Serializes in-process writers so concurrent runs compare and replace safely.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Stable per-process tie-break shared by every observation this process stamps.
static PROCESS_NONCE: OnceLock<String> = OnceLock::new();

/// Process-local order when capture nanoseconds collide inside one process.
static OBSERVATION_SEQ: AtomicU64 = AtomicU64::new(1);

/// Globally sortable observation key.
///
/// Newer observations always sort higher. Equal capture times break ties by
/// process nonce, then by process-local sequence.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ObservationOrder<'a> {
    nanos: u64,
    nonce: &'a str,
    seq: u64,
}

impl<'a> ObservationOrder<'a> {
    fn from_parts(
        observed_at_unix: i64,
        observed_seq: u64,
        observed_at_nanos: u64,
        nonce: &'a str,
    ) -> Self {
        let nanos = if observed_at_nanos > 0 {
            observed_at_nanos
        } else {
            seconds_to_nanos(observed_at_unix)
        };
        Self {
            nanos,
            nonce,
            seq: observed_seq,
        }
    }
}

/// One observed Claude rate-limit reading for a single window.
///
/// Ordering fields carry `#[serde(default)]` so files written before subsecond
/// ordering existed still load: a zero `observed_at_nanos` falls back to
/// `observed_at_unix` seconds, and an empty nonce sorts below any stamped one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RateLimitObservation {
    /// Whole seconds since epoch; used for age display.
    pub(crate) observed_at_unix: i64,
    /// Subsecond capture time as unix-epoch nanoseconds. Zero means legacy.
    #[serde(default)]
    pub(crate) observed_at_nanos: u64,
    /// Process-local order when capture nanoseconds collide.
    #[serde(default)]
    pub(crate) observed_seq: u64,
    /// Per-process nonce (UUID). Empty on legacy files.
    #[serde(default)]
    pub(crate) observed_nonce: String,
    pub(crate) info: RateLimitInfo,
}

impl RateLimitObservation {
    /// Stamp `info` with the current wall clock and a fresh order key.
    pub(crate) fn capture(info: RateLimitInfo) -> Self {
        Self::capture_at_nanos(info, now_unix_nanos())
    }

    pub(crate) fn age_seconds(&self, now_unix: i64) -> i64 {
        now_unix.saturating_sub(self.observed_at_unix).max(0)
    }

    /// Stamp `info` at an explicit unix-epoch nanosecond instant.
    pub(crate) fn capture_at_nanos(info: RateLimitInfo, observed_at_nanos: u64) -> Self {
        Self {
            observed_at_unix: nanos_to_seconds(observed_at_nanos),
            observed_at_nanos,
            observed_seq: next_seq(),
            observed_nonce: process_nonce().to_owned(),
            info,
        }
    }

    /// Build an observation with an explicit order key (tests).
    #[cfg(test)]
    pub(crate) fn with_order(
        info: RateLimitInfo,
        observed_at_nanos: u64,
        observed_seq: u64,
        observed_nonce: impl Into<String>,
    ) -> Self {
        Self {
            observed_at_unix: nanos_to_seconds(observed_at_nanos),
            observed_at_nanos,
            observed_seq,
            observed_nonce: observed_nonce.into(),
            info,
        }
    }

    fn order_key(&self) -> ObservationOrder<'_> {
        ObservationOrder::from_parts(
            self.observed_at_unix,
            self.observed_seq,
            self.observed_at_nanos,
            &self.observed_nonce,
        )
    }

    fn window_key(&self) -> &str {
        self.info.window_key()
    }
}

/// Multi-window Claude rate-limit cache.
///
/// Disk shape is `{ "version": 2, "windows": [ ... ] }`. Legacy
/// single-observation files (`{ "observed_at_unix", "info", ... }`) still load
/// as one window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RateLimitState {
    #[serde(default = "rate_limit_state_version")]
    pub(crate) version: u32,
    pub(crate) windows: Vec<RateLimitObservation>,
}

fn rate_limit_state_version() -> u32 {
    2
}

impl Default for RateLimitState {
    fn default() -> Self {
        Self {
            version: rate_limit_state_version(),
            windows: Vec::new(),
        }
    }
}

impl RateLimitState {
    pub(crate) fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Keep the newer observation per window key.
    pub(crate) fn merge_window(&mut self, observation: RateLimitObservation) {
        let key = observation.window_key().to_owned();
        if let Some(existing) = self
            .windows
            .iter_mut()
            .find(|window| window.window_key() == key)
        {
            if observation.order_key() > existing.order_key() {
                *existing = observation;
            }
            return;
        }
        self.windows.push(observation);
    }

    pub(crate) fn merge_state(&mut self, other: RateLimitState) {
        for window in other.windows {
            self.merge_window(window);
        }
    }

    /// Stable display order: five hour, seven day variants, then the rest.
    pub(crate) fn sorted_windows(&self) -> Vec<&RateLimitObservation> {
        let mut windows: Vec<&RateLimitObservation> = self.windows.iter().collect();
        windows.sort_by(|left, right| {
            window_sort_key(left.window_key())
                .cmp(&window_sort_key(right.window_key()))
                .then_with(|| left.window_key().cmp(right.window_key()))
        });
        windows
    }
}

fn window_sort_key(key: &str) -> u8 {
    match key {
        "five_hour" => 0,
        "seven_day" => 1,
        "seven_day_sonnet" => 2,
        "seven_day_opus" => 3,
        "seven_day_all_models" | "seven_day_all" => 4,
        _ => 50,
    }
}

/// Coalescing multi-window slot. Never drops a newer window under load.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct RateLimitSlot {
    pending: Mutex<RateLimitState>,
}

#[cfg(test)]
impl RateLimitSlot {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Merge one window observation into the pending multi-window state.
    pub(crate) fn publish(&self, observation: RateLimitObservation) {
        let mut guard = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.merge_window(observation);
    }

    /// Take pending windows, clearing the slot.
    pub(crate) fn take(&self) -> Option<RateLimitState> {
        let mut guard = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut *guard))
        }
    }
}

pub(crate) fn default_state_path() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?
        .join("cache")
        .join("claude-code")
        .join(STATE_FILE_NAME))
}

fn legacy_state_path() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join(LEGACY_STATE_FILE_NAME))
}

fn process_nonce() -> &'static str {
    PROCESS_NONCE.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

fn next_seq() -> u64 {
    OBSERVATION_SEQ.fetch_add(1, Ordering::Relaxed)
}

fn lock_path_for(state_path: &Path) -> PathBuf {
    let mut name = state_path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| STATE_FILE_NAME.into());
    name.push(LOCK_FILE_SUFFIX);
    state_path.with_file_name(name)
}

/// Holds the process mutex and exclusive cross-process file lock.
struct StateLock {
    _process_guard: std::sync::MutexGuard<'static, ()>,
    _file_guard: FileLock,
}

fn acquire_state_lock(state_path: &Path) -> anyhow::Result<StateLock> {
    let process_guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_path = lock_path_for(state_path);
    let file = open_lock_file(&lock_path)?;
    Ok(StateLock {
        _process_guard: process_guard,
        _file_guard: FileLock::acquire_with_retry(file, LOCK_RETRY_LIMIT, LOCK_RETRY_DELAY)?,
    })
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    set_private_path_permissions(path)?;
    Ok(file)
}

#[cfg(unix)]
fn set_private_path_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_path_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Persist one window, merging into any multi-window state already on disk.
#[cfg(test)]
pub(crate) fn store_observation(
    path: &Path,
    observation: RateLimitObservation,
) -> anyhow::Result<()> {
    let mut state = RateLimitState::default();
    state.merge_window(observation);
    store_state(path, state)
}

/// Merge `incoming` windows into disk state under the cross-process lock.
pub(crate) fn store_state(path: &Path, incoming: RateLimitState) -> anyhow::Result<()> {
    if incoming.is_empty() {
        return Ok(());
    }
    let _guard = acquire_state_lock(path)?;
    let mut state = load_at(path).unwrap_or_default();
    let writing_default = default_state_path()
        .ok()
        .is_some_and(|default_path| default_path == path);
    // Only the real cache path promotes the old top-level snapshot. Test paths
    // must never read or delete `~/.rho/claude-rate-limit.json`.
    if writing_default && state.is_empty() {
        if let Ok(legacy) = legacy_state_path() {
            if let Some(legacy_state) = load_at(&legacy) {
                state.merge_state(legacy_state);
            }
        }
    }
    state.merge_state(incoming);
    let contents = serde_json::to_vec_pretty(&state)?;
    crate::config_writer::write_bytes_atomically(path, &contents)?;
    if writing_default {
        if let Ok(legacy) = legacy_state_path() {
            let _ = fs::remove_file(&legacy);
            let _ = fs::remove_file(lock_path_for(&legacy));
        }
    }
    Ok(())
}

/// Persist with an explicit order key (tests for equal timestamps / migration).
#[cfg(test)]
pub(crate) fn store_ordered(
    path: &Path,
    info: RateLimitInfo,
    observed_at_nanos: u64,
    observed_seq: u64,
    observed_nonce: impl Into<String>,
) -> anyhow::Result<()> {
    store_observation(
        path,
        RateLimitObservation::with_order(info, observed_at_nanos, observed_seq, observed_nonce),
    )
}

pub(crate) fn load() -> Option<RateLimitState> {
    let path = default_state_path().ok()?;
    if let Some(state) = load_at(&path) {
        return Some(state);
    }
    // One-shot compatibility: read the old top-level file and promote it.
    let legacy = legacy_state_path().ok()?;
    let state = load_at(&legacy)?;
    let _ = store_state(&path, state.clone());
    let _ = fs::remove_file(&legacy);
    let _ = fs::remove_file(lock_path_for(&legacy));
    Some(state)
}

pub(crate) fn load_at(path: &Path) -> Option<RateLimitState> {
    let contents = std::fs::read_to_string(path).ok()?;
    parse_state_json(&contents)
}

fn parse_state_json(contents: &str) -> Option<RateLimitState> {
    if let Ok(mut state) = serde_json::from_str::<RateLimitState>(contents) {
        if state.version >= 2 || !state.windows.is_empty() {
            if state.version == 0 {
                state.version = rate_limit_state_version();
            }
            return Some(state);
        }
    }
    // Legacy single-observation file written before multi-window state.
    let observation = serde_json::from_str::<RateLimitObservation>(contents).ok()?;
    let mut state = RateLimitState::default();
    state.merge_window(observation);
    Some(state)
}

pub(crate) fn now_unix() -> i64 {
    nanos_to_seconds(now_unix_nanos())
}

fn now_unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn seconds_to_nanos(seconds: i64) -> u64 {
    u64::try_from(seconds.max(0))
        .unwrap_or(0)
        .saturating_mul(1_000_000_000)
}

fn nanos_to_seconds(nanos: u64) -> i64 {
    i64::try_from(nanos / 1_000_000_000).unwrap_or(i64::MAX)
}

/// Formats a non-negative age. Sentinel timestamps (`<= 0`) must not be passed
/// in; use [`format_age_since`] so those cannot render as multi-decade ages.
pub(crate) fn format_age(seconds: i64) -> String {
    if seconds < 0 {
        return "0s ago".into();
    }
    if seconds < 60 {
        return format!("{seconds}s ago");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

pub(crate) fn format_age_since(at_unix: i64, now_unix: i64) -> Option<String> {
    if at_unix <= 0 {
        return None;
    }
    Some(format_age(now_unix.saturating_sub(at_unix).max(0)))
}

#[cfg(test)]
#[path = "rate_limit_tests.rs"]
mod tests;
