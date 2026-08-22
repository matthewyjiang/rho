//! Gitignore-aware bounded walks over a resolved workspace root.
//!
//! This is the canonical workspace walker. Ignore rules, symlink policy, and
//! `.git` filtering are shared. [`visit_files`] is the serial, name-sorted
//! walk used by `glob` and the TUI file picker. [`visit_files_parallel`] is
//! the overlapping walk used by `grep`; callers sort collected hits before
//! truncating so output stays deterministic.
//!
//! **Symlink security:** walks never follow symbolic links
//! (`WalkBuilder::follow_links(false)`). Only regular files are yielded. A
//! symlink inside an authorized root must not open a path outside that root,
//! because search tools request one `read_path` grant for the tree rather than
//! per file. Do not flip `follow_links` without revisiting that grant model.

use std::{
    ops::ControlFlow,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use ignore::{WalkBuilder, WalkState};

/// Walk bound: stop after inspecting this many directory entries.
pub const MAX_ENTRIES_SCANNED: usize = 200_000;

/// Whether a walk descends into dot-files and dot-directories.
///
/// The root itself is always entered, so a walk explicitly scoped to a hidden
/// directory still yields its contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HiddenFiles {
    Include,
    Skip,
}

/// Bounds that stop a walk before it can dominate a tool call.
#[derive(Clone, Debug)]
pub struct WalkLimits {
    pub max_entries: usize,
    pub deadline: Instant,
}

impl WalkLimits {
    /// Standard bounds: [`MAX_ENTRIES_SCANNED`] entries within `budget`.
    pub fn within(budget: Duration) -> Self {
        Self {
            max_entries: MAX_ENTRIES_SCANNED,
            deadline: Instant::now() + budget,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WalkOptions {
    pub hidden: HiddenFiles,
    pub limits: WalkLimits,
}

/// A regular file discovered by the walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkedFile {
    pub absolute: PathBuf,
    /// Root-relative, `/`-separated, for display and glob matching.
    pub relative: String,
}

/// Why a walk ended. Reported to the model so a capped result is never
/// mistaken for a complete one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkStop {
    Completed,
    EntryLimit,
    Deadline,
    ResultLimit,
    Cancelled,
}

/// Walks `root` honoring `.gitignore`/`.ignore`, never following symlinks,
/// yielding regular files only. The visitor may end the walk early.
///
/// Entries are sorted by name within each directory, so the walk order is
/// stable across runs and platforms. Callers that cap results therefore emit a
/// reproducible prefix rather than an arbitrary sample, and need no further
/// sorting.
pub fn visit_files(
    root: &Path,
    options: &WalkOptions,
    mut visit: impl FnMut(WalkedFile) -> ControlFlow<WalkStop>,
) -> WalkStop {
    let mut builder = walk_builder(root, options);
    let walker = builder.sort_by_file_name(std::ffi::OsStr::cmp).build();

    let mut entries_seen = 0usize;
    for entry in walker {
        if Instant::now() >= options.limits.deadline {
            return WalkStop::Deadline;
        }

        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > options.limits.max_entries {
            return WalkStop::EntryLimit;
        }

        let Some(file) = walked_file(root, entry) else {
            continue;
        };

        match visit(file) {
            ControlFlow::Continue(()) => {}
            ControlFlow::Break(stop) => return stop,
        }
    }

    WalkStop::Completed
}

/// Parallel counterpart of [`visit_files`] for overlapping per-file work.
///
/// Ignore rules, symlink policy, hidden-file handling, and `.git` filtering
/// match [`visit_files`]. Discovery order is not sorted: `ignore`'s parallel
/// walker does not honor `sort_by_file_name`. Callers must collect hits,
/// sort by relative path, then apply `max_results` truncation so output is
/// identical across runs.
///
/// Caps (`max_entries`, deadline, visitor `Break`) set a shared stop flag
/// that workers check before the next entry. `WalkState::Quit` is
/// asynchronous, so in-flight visitors may still produce extra hits past a
/// cap; `total_matches` can therefore exceed the serial walk. Stop reasons
/// use the highest-priority observed terminal condition: cancelled, then
/// deadline, entry limit, result limit, else completed.
pub fn visit_files_parallel(
    root: &Path,
    options: &WalkOptions,
    visit: impl Fn(WalkedFile) -> ControlFlow<WalkStop> + Send + Sync,
) -> WalkStop {
    let walker = walk_builder(root, options).build_parallel();
    let stop = AtomicU8::new(STOP_RUNNING);
    let entries_seen = AtomicUsize::new(0);

    walker.run(|| {
        Box::new(|entry| {
            if current_stop(&stop).is_some() {
                return WalkState::Quit;
            }
            if Instant::now() >= options.limits.deadline {
                record_stop(&stop, WalkStop::Deadline);
                return WalkState::Quit;
            }

            let seen = entries_seen.fetch_add(1, Ordering::Relaxed) + 1;
            if seen > options.limits.max_entries {
                record_stop(&stop, WalkStop::EntryLimit);
                return WalkState::Quit;
            }

            let Some(file) = walked_file(root, entry) else {
                return WalkState::Continue;
            };

            match visit(file) {
                ControlFlow::Continue(()) => WalkState::Continue,
                ControlFlow::Break(reason) => {
                    record_stop(&stop, reason);
                    WalkState::Quit
                }
            }
        })
    });

    current_stop(&stop).unwrap_or(WalkStop::Completed)
}

fn walk_builder(root: &Path, options: &WalkOptions) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .follow_links(false)
        .require_git(false)
        .hidden(matches!(options.hidden, HiddenFiles::Skip))
        // Depth 0 is the requested root, which the caller has already chosen;
        // only its descendants are filtered.
        .filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".git");
    builder
}

fn walked_file(root: &Path, entry: Result<ignore::DirEntry, ignore::Error>) -> Option<WalkedFile> {
    let entry = entry.ok()?;
    if !entry.file_type().is_some_and(|ty| ty.is_file()) {
        return None;
    }
    let absolute = entry.into_path();
    let relative = relative_path(root, &absolute)?;
    if relative.is_empty() {
        return None;
    }
    Some(WalkedFile { absolute, relative })
}

/// Sentinel stored in the parallel stop flag before any worker finishes.
const STOP_RUNNING: u8 = 0;

fn stop_rank(stop: WalkStop) -> u8 {
    match stop {
        WalkStop::Completed => 0,
        WalkStop::ResultLimit => 1,
        WalkStop::EntryLimit => 2,
        WalkStop::Deadline => 3,
        WalkStop::Cancelled => 4,
    }
}

fn stop_from_rank(rank: u8) -> Option<WalkStop> {
    match rank {
        1 => Some(WalkStop::ResultLimit),
        2 => Some(WalkStop::EntryLimit),
        3 => Some(WalkStop::Deadline),
        4 => Some(WalkStop::Cancelled),
        _ => None,
    }
}

fn record_stop(slot: &AtomicU8, stop: WalkStop) {
    let rank = stop_rank(stop);
    slot.fetch_max(rank, Ordering::Relaxed);
}

fn current_stop(slot: &AtomicU8) -> Option<WalkStop> {
    stop_from_rank(slot.load(Ordering::Relaxed))
}

fn relative_path(root: &Path, absolute: &Path) -> Option<String> {
    let relative = absolute.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
#[path = "workspace_walk_tests.rs"]
mod tests;
