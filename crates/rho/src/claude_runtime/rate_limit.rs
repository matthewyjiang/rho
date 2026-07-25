//! Persist the latest Claude Code rate-limit observation for `/limits`.
//!
//! This is not a credential path. Rho never stores Claude tokens; it only
//! remembers what a prior stream reported.
//!
//! Ordering is wall-clock seconds plus a process-wide monotonic sequence so
//! concurrent runs and equal timestamps cannot let an older observation win.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::stream::{describe_rate_limit, RateLimitInfo};

const STATE_FILE_NAME: &str = "claude-rate-limit.json";

/// Serializes in-process writers so concurrent runs compare and replace safely.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Process-wide tie-break when wall-clock seconds collide.
static OBSERVATION_SEQ: AtomicU64 = AtomicU64::new(1);

/// Observed Claude rate-limit info plus when Rho saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ObservedRateLimit {
    pub(crate) observed_at_unix: i64,
    /// Monotonic process order captured with the observation. Missing on files
    /// written before this field existed; treated as zero for comparison.
    #[serde(default)]
    pub(crate) observed_seq: u64,
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

    fn order_key(&self) -> (i64, u64) {
        (self.observed_at_unix, self.observed_seq)
    }
}

/// Observation ready for durable store, ordered by capture time + sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RateLimitObservation {
    pub(crate) observed_at_unix: i64,
    pub(crate) observed_seq: u64,
    pub(crate) info: RateLimitInfo,
}

impl RateLimitObservation {
    /// Stamp `info` with the current wall clock and a fresh sequence number.
    pub(crate) fn capture(info: RateLimitInfo) -> Self {
        Self::capture_at(info, now_unix())
    }

    /// Stamp `info` at `observed_at_unix` with a fresh sequence number.
    pub(crate) fn capture_at(info: RateLimitInfo, observed_at_unix: i64) -> Self {
        Self {
            observed_at_unix,
            observed_seq: next_seq(),
            info,
        }
    }

    /// Build an observation with an explicit order key (tests).
    #[cfg(test)]
    pub(crate) fn with_order(
        info: RateLimitInfo,
        observed_at_unix: i64,
        observed_seq: u64,
    ) -> Self {
        Self {
            observed_at_unix,
            observed_seq,
            info,
        }
    }

    fn order_key(&self) -> (i64, u64) {
        (self.observed_at_unix, self.observed_seq)
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

fn next_seq() -> u64 {
    OBSERVATION_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Persist a fully ordered observation if no newer observation exists on disk.
pub(crate) fn store_observation(
    path: &Path,
    observation: RateLimitObservation,
) -> anyhow::Result<()> {
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = load_at(path) {
        if existing.order_key() >= observation.order_key() {
            return Ok(());
        }
    }
    let observed = ObservedRateLimit {
        observed_at_unix: observation.observed_at_unix,
        observed_seq: observation.observed_seq,
        info: observation.info,
    };
    let contents = serde_json::to_vec_pretty(&observed)?;
    crate::config_writer::write_bytes_atomically(path, &contents)?;
    Ok(())
}

/// Persist with an explicit order key (tests for equal timestamps).
#[cfg(test)]
pub(crate) fn store_ordered(
    path: &Path,
    info: RateLimitInfo,
    observed_at_unix: i64,
    observed_seq: u64,
) -> anyhow::Result<()> {
    store_observation(
        path,
        RateLimitObservation::with_order(info, observed_at_unix, observed_seq),
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
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
