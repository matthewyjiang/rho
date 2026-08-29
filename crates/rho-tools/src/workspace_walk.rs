//! Gitignore-aware bounded walks over a resolved workspace root.
//!
//! This is the canonical workspace walker. The `grep` and `glob` tools and the
//! TUI file picker all go through [`visit_files`] so ignore rules, symlink
//! policy, and ordering stay identical everywhere.
//!
//! **Symlink security:** walks never follow symbolic links
//! (`WalkBuilder::follow_links(false)`). Only regular files are yielded. A
//! symlink inside an authorized root must not open a path outside that root,
//! because search tools request one `read_path` grant for the tree rather than
//! per file. Do not flip `follow_links` without revisiting that grant model.

use std::{
    ops::ControlFlow,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use ignore::WalkBuilder;

/// Walk bound: stop after inspecting this many directory entries.
pub const MAX_ENTRIES_SCANNED: usize = 200_000;

/// Whether a walk descends into dot-files and dot-directories.
///
/// The root itself is always entered, so a walk explicitly scoped to a hidden
/// directory still yields its contents, and a walk scoped to a file yields
/// that file even when it is hidden or ignored.
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
    /// Empty when `root` itself is the file.
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
/// A file root yields that file and does not apply ignore or hidden-file
/// skipping. The caller named it. Directory walks still filter descendants.
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
    if let Some(file) = root_file(root) {
        if Instant::now() >= options.limits.deadline {
            return WalkStop::Deadline;
        }
        return match visit(file) {
            ControlFlow::Continue(()) => WalkStop::Completed,
            ControlFlow::Break(stop) => stop,
        };
    }

    let mut builder = WalkBuilder::new(root);
    builder
        .follow_links(false)
        .require_git(false)
        .hidden(matches!(options.hidden, HiddenFiles::Skip))
        // Depth 0 is the requested root, which the caller has already chosen;
        // only its descendants are filtered.
        .filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".git");
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

fn root_file(root: &Path) -> Option<WalkedFile> {
    let metadata = std::fs::symlink_metadata(root).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    Some(WalkedFile {
        absolute: root.to_path_buf(),
        relative: String::new(),
    })
}

fn walked_file(root: &Path, entry: Result<ignore::DirEntry, ignore::Error>) -> Option<WalkedFile> {
    let entry = entry.ok()?;
    if !entry.file_type().is_some_and(|ty| ty.is_file()) {
        return None;
    }
    let absolute = entry.into_path();
    let relative = relative_path(root, &absolute)?;
    Some(WalkedFile { absolute, relative })
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
