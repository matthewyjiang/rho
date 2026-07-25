//! Persist the latest Claude Code rate-limit observation for `/limits`.
//!
//! This is not a credential path. Rho never stores Claude tokens; it only
//! remembers what a prior stream reported.
//!
//! Ordering is unix-epoch nanoseconds plus a per-process nonce and monotonic
//! sequence so concurrent Rho processes and equal timestamps cannot let an
//! older observation win. Compare-and-replace runs under a cross-process lock.

use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::stream::{describe_rate_limit, RateLimitInfo};

const STATE_FILE_NAME: &str = "claude-rate-limit.json";
const LOCK_FILE_SUFFIX: &str = ".lock";
const LOCK_RETRY_LIMIT: u32 = 1_000;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);

/// Serializes in-process writers so concurrent runs compare and replace safely.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Stable per-process tie-break shared by every observation this process stamps.
static PROCESS_NONCE: OnceLock<String> = OnceLock::new();

/// Process-local order when capture nanoseconds collide inside one process.
static OBSERVATION_SEQ: AtomicU64 = AtomicU64::new(1);

/// Observed Claude rate-limit info plus when Rho saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ObservedRateLimit {
    /// Whole seconds since epoch; kept for age display and legacy files.
    pub(crate) observed_at_unix: i64,
    /// Legacy process-local sequence. Missing on older files; treated as zero.
    /// Used for order only when `observed_at_nanos` is absent.
    #[serde(default)]
    pub(crate) observed_seq: u64,
    /// Subsecond capture time as unix-epoch nanoseconds. Zero means legacy:
    /// derive nanoseconds from `observed_at_unix` seconds.
    #[serde(default)]
    pub(crate) observed_at_nanos: u64,
    /// Per-process nonce (UUID). Empty on legacy files.
    #[serde(default)]
    pub(crate) observed_nonce: String,
    pub(crate) info: RateLimitInfo,
}

impl ObservedRateLimit {
    pub(crate) fn age_seconds(&self, now_unix: i64) -> i64 {
        now_unix.saturating_sub(self.observed_at_unix).max(0)
    }

    /// One-line display for `/limits`: window, status, reset, age. No percent.
    pub(crate) fn describe(&self, now_unix: i64) -> String {
        let body = describe_rate_limit(&self.info);
        let age = format_age(self.age_seconds(now_unix));
        format!("claude code: {body} (last observed {age})")
    }

    fn order_key(&self) -> ObservationOrder {
        ObservationOrder::from_parts(
            self.observed_at_unix,
            self.observed_seq,
            self.observed_at_nanos,
            &self.observed_nonce,
        )
    }
}

/// Globally sortable observation key.
///
/// Newer observations always sort higher. Equal capture times break ties by
/// process nonce, then by process-local sequence.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ObservationOrder {
    nanos: u64,
    nonce: String,
    seq: u64,
}

impl ObservationOrder {
    fn from_parts(
        observed_at_unix: i64,
        observed_seq: u64,
        observed_at_nanos: u64,
        nonce: &str,
    ) -> Self {
        let nanos = if observed_at_nanos > 0 {
            observed_at_nanos
        } else {
            seconds_to_nanos(observed_at_unix)
        };
        Self {
            nanos,
            nonce: nonce.to_owned(),
            seq: observed_seq,
        }
    }
}

/// Observation ready for durable store, ordered by capture time + tie-break.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RateLimitObservation {
    pub(crate) observed_at_unix: i64,
    pub(crate) observed_at_nanos: u64,
    pub(crate) observed_seq: u64,
    pub(crate) observed_nonce: String,
    pub(crate) info: RateLimitInfo,
}

impl RateLimitObservation {
    /// Stamp `info` with the current wall clock and a fresh order key.
    pub(crate) fn capture(info: RateLimitInfo) -> Self {
        Self::capture_at_nanos(info, now_unix_nanos())
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

    fn order_key(&self) -> ObservationOrder {
        ObservationOrder::from_parts(
            self.observed_at_unix,
            self.observed_seq,
            self.observed_at_nanos,
            &self.observed_nonce,
        )
    }
}

/// Coalescing latest-value slot. Never drops a newer observation under load.
#[derive(Default)]
pub(crate) struct RateLimitSlot {
    latest: Mutex<Option<RateLimitObservation>>,
}

impl RateLimitSlot {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Keep the observation with the highest order key.
    pub(crate) fn publish(&self, observation: RateLimitObservation) {
        let mut guard = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.as_ref() {
            Some(existing) if existing.order_key() >= observation.order_key() => {}
            _ => *guard = Some(observation),
        }
    }

    /// Take the latest observation, clearing the slot.
    pub(crate) fn take(&self) -> Option<RateLimitObservation> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

pub(crate) fn default_state_path() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join(STATE_FILE_NAME))
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
    file: File,
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self.file);
    }
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
    lock_exclusive_with_retry(&file)?;
    Ok(StateLock {
        _process_guard: process_guard,
        file,
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

fn lock_exclusive_with_retry(file: &File) -> io::Result<()> {
    let mut attempts = 0;
    loop {
        match try_lock_exclusive(file) {
            Ok(()) => return Ok(()),
            Err(error) if is_lock_busy(&error) && attempts < LOCK_RETRY_LIMIT => {
                attempts += 1;
                thread::sleep(LOCK_RETRY_DELAY);
            }
            Err(error)
                if error.kind() == io::ErrorKind::Interrupted && attempts < LOCK_RETRY_LIMIT =>
            {
                attempts += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn try_lock_exclusive(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(());
        }
        Err(io::Error::last_os_error())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::GetLastError,
            Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY},
            System::IO::OVERLAPPED,
        };

        let mut overlapped = OVERLAPPED::default();
        let result = unsafe {
            LockFileEx(
                file.as_raw_handle(),
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                1,
                0,
                &mut overlapped,
            )
        };
        if result != 0 {
            return Ok(());
        }
        let err = unsafe { GetLastError() };
        Err(io::Error::from_raw_os_error(err as i32))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Ok(())
    }
}

fn unlock_file(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{Storage::FileSystem::UnlockFileEx, System::IO::OVERLAPPED};

        let mut overlapped = OVERLAPPED::default();
        let result = unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Ok(())
    }
}

fn is_lock_busy(error: &io::Error) -> bool {
    match error.kind() {
        io::ErrorKind::WouldBlock => true,
        // Windows ERROR_LOCK_VIOLATION / ERROR_BUSY often map as Other.
        _ => {
            #[cfg(windows)]
            {
                const ERROR_LOCK_VIOLATION: i32 = 33;
                const ERROR_BUSY: i32 = 170;
                matches!(
                    error.raw_os_error(),
                    Some(ERROR_LOCK_VIOLATION | ERROR_BUSY)
                )
            }
            #[cfg(unix)]
            {
                // EWOULDBLOCK and EAGAIN are identical on many unix targets.
                matches!(
                    error.raw_os_error(),
                    Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
                )
            }
            #[cfg(not(any(unix, windows)))]
            {
                false
            }
        }
    }
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

/// Persist a fully ordered observation if no newer observation exists on disk.
pub(crate) fn store_observation(
    path: &Path,
    observation: RateLimitObservation,
) -> anyhow::Result<()> {
    let _guard = acquire_state_lock(path)?;
    if let Some(existing) = load_at(path) {
        if existing.order_key() >= observation.order_key() {
            return Ok(());
        }
    }
    let observed = ObservedRateLimit {
        observed_at_unix: observation.observed_at_unix,
        observed_seq: observation.observed_seq,
        observed_at_nanos: observation.observed_at_nanos,
        observed_nonce: observation.observed_nonce,
        info: observation.info,
    };
    let contents = serde_json::to_vec_pretty(&observed)?;
    crate::config_writer::write_bytes_atomically(path, &contents)?;
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

pub(crate) fn load() -> Option<ObservedRateLimit> {
    load_at(&default_state_path().ok()?)
}

pub(crate) fn load_at(path: &Path) -> Option<ObservedRateLimit> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
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

fn format_age(seconds: i64) -> String {
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

#[cfg(test)]
#[path = "rate_limit_tests.rs"]
mod tests;
