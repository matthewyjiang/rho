//! Last-successful OAuth usage windows for `/limits`.
//!
//! This is a stale-while-revalidate snapshot: the overlay may show it
//! immediately, then replace rows as live fetches complete. Errors are never
//! stored as usage. Claude Code observations live in the claude-code cache,
//! not here.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::usage_limits::{UsageLimitWindow, UsageProviderKind};

const CACHE_VERSION: u32 = 1;
const STATE_FILE_NAME: &str = "oauth-usage-limits.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageLimitsCache {
    #[serde(default = "cache_version")]
    pub version: u32,
    #[serde(default)]
    pub providers: Vec<CachedProviderLimits>,
}

impl Default for UsageLimitsCache {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            providers: Vec::new(),
        }
    }
}

fn cache_version() -> u32 {
    CACHE_VERSION
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CachedProviderLimits {
    pub provider: String,
    pub fetched_at_unix: i64,
    pub windows: Vec<UsageLimitWindow>,
}

impl UsageLimitsCache {
    pub fn get(&self, kind: UsageProviderKind) -> Option<&CachedProviderLimits> {
        let label = kind.label();
        self.providers.iter().find(|entry| entry.provider == label)
    }

    pub fn upsert(
        &mut self,
        kind: UsageProviderKind,
        windows: Vec<UsageLimitWindow>,
        fetched_at_unix: i64,
    ) {
        let label = kind.label().to_string();
        if let Some(existing) = self
            .providers
            .iter_mut()
            .find(|entry| entry.provider == label)
        {
            existing.windows = windows;
            existing.fetched_at_unix = fetched_at_unix;
            return;
        }
        self.providers.push(CachedProviderLimits {
            provider: label,
            fetched_at_unix,
            windows,
        });
        self.providers
            .sort_by(|left, right| left.provider.cmp(&right.provider));
    }
}

pub fn default_path() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join("cache").join(STATE_FILE_NAME))
}

pub fn load() -> UsageLimitsCache {
    default_path()
        .ok()
        .and_then(|path| load_from(&path).ok())
        .unwrap_or_default()
}

pub fn load_from(path: &Path) -> io::Result<UsageLimitsCache> {
    let bytes = fs::read(path)?;
    let cache = serde_json::from_slice::<UsageLimitsCache>(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if cache.version != CACHE_VERSION {
        return Ok(UsageLimitsCache::default());
    }
    Ok(cache)
}

pub fn save(cache: &UsageLimitsCache) -> io::Result<()> {
    let path = default_path().map_err(io::Error::other)?;
    save_to(&path, cache)
}

pub fn save_to(path: &Path, cache: &UsageLimitsCache) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let encoded = serde_json::to_vec_pretty(cache)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
#[path = "usage_limits_cache_tests.rs"]
mod tests;
