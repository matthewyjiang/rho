//! Gitignore-aware bounded walks over a resolved workspace root.
//!
//! **Symlink security:** walks never follow symbolic links
//! (`WalkBuilder::follow_links(false)`). Only regular files are yielded. A
//! symlink inside an authorized root must not open a path outside that root,
//! because search tools request one `read_path` grant for the tree rather than
//! per file. Do not flip `follow_links` without revisiting that grant model.

use std::{
    ops::ControlFlow,
    path::{Component, Path, PathBuf},
    time::Instant,
};

use ignore::WalkBuilder;

/// Whether a walk descends into dot-files and dot-directories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HiddenFiles {
    Include,
    Skip,
}

/// Bounds that stop a walk before it can dominate a tool call.
#[derive(Clone, Debug)]
pub(crate) struct WalkLimits {
    pub max_entries: usize,
    pub deadline: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct WalkOptions {
    pub hidden: HiddenFiles,
    pub limits: WalkLimits,
}

/// A regular file discovered by the walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WalkedFile {
    pub absolute: PathBuf,
    /// Root-relative, `/`-separated, for display and glob matching.
    pub relative: String,
}

/// Why a walk ended. Reported to the model so a capped result is never
/// mistaken for a complete one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WalkStop {
    Completed,
    EntryLimit,
    Deadline,
    ResultLimit,
    Cancelled,
}

/// Walks `root` honoring `.gitignore`/`.ignore`, never following symlinks,
/// yielding regular files only. The visitor may end the walk early.
pub(crate) fn visit_files(
    root: &Path,
    options: &WalkOptions,
    mut visit: impl FnMut(WalkedFile) -> ControlFlow<WalkStop>,
) -> WalkStop {
    let walker = WalkBuilder::new(root)
        .follow_links(false)
        .require_git(false)
        .hidden(matches!(options.hidden, HiddenFiles::Skip))
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();

    let mut entries_seen = 0usize;
    for entry in walker {
        if Instant::now() >= options.limits.deadline {
            return WalkStop::Deadline;
        }

        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > options.limits.max_entries {
            return WalkStop::EntryLimit;
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };

        if !entry.file_type().is_some_and(|ty| ty.is_file()) {
            continue;
        }

        let absolute = entry.into_path();
        let Some(relative) = relative_path(root, &absolute) else {
            continue;
        };
        if relative.is_empty() {
            continue;
        }

        match visit(WalkedFile { absolute, relative }) {
            ControlFlow::Continue(()) => {}
            ControlFlow::Break(stop) => return stop,
        }
    }

    WalkStop::Completed
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
