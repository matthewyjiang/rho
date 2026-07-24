//! Persist the latest Claude Code rate-limit observation for `/limits`.
//!
//! This is not a credential path. Rho never stores Claude tokens; it only
//! remembers what a prior stream reported.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::stream::{describe_rate_limit, RateLimitInfo};

const STATE_FILE_NAME: &str = "claude-rate-limit.json";

/// Serializes in-process writers so concurrent runs compare and replace safely.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Observed Claude rate-limit info plus when Rho saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ObservedRateLimit {
    pub(crate) observed_at_unix: i64,
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
}

fn state_path() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join(STATE_FILE_NAME))
}

pub(crate) fn store(info: RateLimitInfo) -> anyhow::Result<()> {
    store_at(&state_path()?, info, now_unix())
}

/// Persist `info` observed at `observed_at_unix` if no newer observation exists.
pub(crate) fn store_at(
    path: &Path,
    info: RateLimitInfo,
    observed_at_unix: i64,
) -> anyhow::Result<()> {
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = load_at(path) {
        if existing.observed_at_unix > observed_at_unix {
            return Ok(());
        }
    }
    let observed = ObservedRateLimit {
        observed_at_unix,
        info,
    };
    let contents = serde_json::to_vec_pretty(&observed)?;
    crate::config_writer::write_bytes_atomically(path, &contents)?;
    Ok(())
}

pub(crate) fn load() -> Option<ObservedRateLimit> {
    load_at(&state_path().ok()?)
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
