//! Session-scoped full-file snapshots for hashline recovery and seen-line checks.
//!
//! Producers (`read_file`, `grep`, `write_file`, `edit`) call [`SnapshotStore::record`]
//! with the full text they observed and the 1-indexed lines they actually showed.
//! Consumers resolve a stale section tag back to that text and remap anchors, or
//! reject edits that touch lines the model never saw under this tag.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
};

use super::format::compute_file_hash;

/// Default distinct paths retained in one session store.
const DEFAULT_MAX_PATHS: usize = 30;
/// Full-file versions kept per path (oldest dropped first).
const DEFAULT_MAX_VERSIONS_PER_PATH: usize = 4;
/// Global ceiling on retained snapshot text bytes across every path.
const DEFAULT_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// One full-file version observed during the session.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub path: PathBuf,
    pub text: String,
    pub hash: String,
    /// 1-indexed lines a producer displayed under this tag. `None` means no
    /// provenance was recorded, so seen-line checks are skipped.
    pub seen_lines: Option<HashSet<usize>>,
}

/// Process-local counters for hashline soft-recovery and guardrails.
#[derive(Debug, Default)]
pub struct HashlineMetrics {
    pub tag_mismatch: AtomicU64,
    pub recovery_ok: AtomicU64,
    pub recovery_fail: AtomicU64,
    pub unseen_reject: AtomicU64,
}

impl HashlineMetrics {
    pub fn snapshot(&self) -> HashlineMetricsSnapshot {
        HashlineMetricsSnapshot {
            tag_mismatch: self.tag_mismatch.load(Ordering::Relaxed),
            recovery_ok: self.recovery_ok.load(Ordering::Relaxed),
            recovery_fail: self.recovery_fail.load(Ordering::Relaxed),
            unseen_reject: self.unseen_reject.load(Ordering::Relaxed),
        }
    }
}

/// Point-in-time copy of [`HashlineMetrics`] for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HashlineMetricsSnapshot {
    pub tag_mismatch: u64,
    pub recovery_ok: u64,
    pub recovery_fail: u64,
    pub unseen_reject: u64,
}

/// Shared per-session snapshot ring used by coding tools.
#[derive(Debug)]
pub struct SnapshotStore {
    inner: Mutex<Inner>,
    metrics: HashlineMetrics,
}

#[derive(Debug)]
struct Inner {
    histories: HashMap<PathBuf, VecDeque<Snapshot>>,
    /// Least-recently-used path order: front is most recent.
    lru: VecDeque<PathBuf>,
    total_bytes: usize,
    max_paths: usize,
    max_versions_per_path: usize,
    max_total_bytes: usize,
}

impl Default for SnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self::with_limits(
            DEFAULT_MAX_PATHS,
            DEFAULT_MAX_VERSIONS_PER_PATH,
            DEFAULT_MAX_TOTAL_BYTES,
        )
    }

    pub fn with_limits(
        max_paths: usize,
        max_versions_per_path: usize,
        max_total_bytes: usize,
    ) -> Self {
        Self {
            inner: Mutex::new(Inner {
                histories: HashMap::new(),
                lru: VecDeque::new(),
                total_bytes: 0,
                max_paths: max_paths.max(1),
                max_versions_per_path: max_versions_per_path.max(1),
                max_total_bytes: max_total_bytes.max(1),
            }),
            metrics: HashlineMetrics::default(),
        }
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    pub fn metrics(&self) -> &HashlineMetrics {
        &self.metrics
    }

    /// Most recently recorded version for `path`, if any.
    pub fn head(&self, path: &Path) -> Option<Snapshot> {
        let mut inner = self.inner.lock().expect("snapshot store lock");
        inner.touch(path);
        inner.histories.get(path).and_then(|h| h.front().cloned())
    }

    /// Version for `path` whose tag equals `hash` (case-insensitive). On short
    /// tag collisions, the most recently recorded matching version wins.
    pub fn by_hash(&self, path: &Path, hash: &str) -> Option<Snapshot> {
        let mut inner = self.inner.lock().expect("snapshot store lock");
        inner.touch(path);
        inner.histories.get(path).and_then(|history| {
            history
                .iter()
                .find(|snap| snap.hash.eq_ignore_ascii_case(hash))
                .cloned()
        })
    }

    /// Version whose text equals `full_text` byte-for-byte.
    pub fn by_content(&self, path: &Path, full_text: &str) -> Option<Snapshot> {
        let mut inner = self.inner.lock().expect("snapshot store lock");
        inner.touch(path);
        inner
            .histories
            .get(path)
            .and_then(|history| history.iter().find(|snap| snap.text == full_text).cloned())
    }

    /// Every retained version whose tag equals `hash`, across all paths.
    pub fn find_by_hash(&self, hash: &str) -> Vec<Snapshot> {
        let inner = self.inner.lock().expect("snapshot store lock");
        let mut matches = Vec::new();
        for history in inner.histories.values() {
            for snap in history {
                if snap.hash.eq_ignore_ascii_case(hash) {
                    matches.push(snap.clone());
                }
            }
        }
        matches
    }

    /// Record full file text and optional displayed line numbers. Returns the tag.
    pub fn record(
        &self,
        path: impl Into<PathBuf>,
        full_text: impl Into<String>,
        seen_lines: Option<impl IntoIterator<Item = usize>>,
    ) -> String {
        let path = path.into();
        let full_text = full_text.into();
        let hash = compute_file_hash(&full_text);
        let seen = seen_lines.map(|lines| lines.into_iter().collect::<HashSet<_>>());

        let mut inner = self.inner.lock().expect("snapshot store lock");
        let max_versions = inner.max_versions_per_path;

        // Dedup on full text, not tag alone: 4-hex collisions must stay distinct.
        {
            let history = inner.histories.entry(path.clone()).or_default();
            if let Some(pos) = history.iter().position(|snap| snap.text == full_text) {
                let mut existing = history.remove(pos).expect("index in range");
                existing.hash = hash.clone();
                merge_seen(&mut existing.seen_lines, seen.as_ref());
                history.push_front(existing);
                inner.touch(&path);
                return hash;
            }
            let snapshot = Snapshot {
                path: path.clone(),
                text: full_text,
                hash: hash.clone(),
                seen_lines: seen,
            };
            let added = snapshot.text.len();
            history.push_front(snapshot);
            let mut dropped_bytes = 0usize;
            while history.len() > max_versions {
                if let Some(dropped) = history.pop_back() {
                    dropped_bytes = dropped_bytes.saturating_add(dropped.text.len());
                }
            }
            inner.total_bytes = inner
                .total_bytes
                .saturating_add(added)
                .saturating_sub(dropped_bytes);
        }
        inner.touch(&path);
        inner.evict_if_needed();
        hash
    }

    /// Merge displayed lines into an existing version identified by tag.
    pub fn record_seen_lines(
        &self,
        path: &Path,
        hash: &str,
        lines: impl IntoIterator<Item = usize>,
    ) {
        let lines: HashSet<usize> = lines.into_iter().collect();
        if lines.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().expect("snapshot store lock");
        if let Some(history) = inner.histories.get_mut(path) {
            if let Some(snap) = history
                .iter_mut()
                .find(|snap| snap.hash.eq_ignore_ascii_case(hash))
            {
                merge_seen(&mut snap.seen_lines, Some(&lines));
            }
        }
        inner.touch(path);
    }

    pub fn invalidate(&self, path: &Path) {
        let mut inner = self.inner.lock().expect("snapshot store lock");
        if let Some(history) = inner.histories.remove(path) {
            for snap in history {
                inner.total_bytes = inner.total_bytes.saturating_sub(snap.text.len());
            }
        }
        inner.lru.retain(|p| p != path);
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock().expect("snapshot store lock");
        inner.histories.clear();
        inner.lru.clear();
        inner.total_bytes = 0;
    }
}

impl Inner {
    fn touch(&mut self, path: &Path) {
        self.lru.retain(|p| p != path);
        self.lru.push_front(path.to_path_buf());
    }

    fn evict_if_needed(&mut self) {
        while self.histories.len() > self.max_paths || self.total_bytes > self.max_total_bytes {
            let Some(old) = self.lru.pop_back() else {
                break;
            };
            if let Some(history) = self.histories.remove(&old) {
                for snap in history {
                    self.total_bytes = self.total_bytes.saturating_sub(snap.text.len());
                }
            }
        }
    }
}

fn merge_seen(target: &mut Option<HashSet<usize>>, extra: Option<&HashSet<usize>>) {
    let Some(extra) = extra else {
        return;
    };
    match target {
        Some(set) => set.extend(extra.iter().copied()),
        None => *target = Some(extra.clone()),
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
