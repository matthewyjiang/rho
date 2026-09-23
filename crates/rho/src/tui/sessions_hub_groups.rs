//! `/sessions` hub grouping: sessions by directory, directories by repository.
//!
//! Worktrees of one Git repository share a [`RepoGroup`] so a repo with many
//! checkouts reads as one block instead of one top-level section per checkout.
//! Deleted directories cannot name their repository, so they collect in one
//! collapsed group at the end. Placement comes from an injected resolver,
//! keeping this module free of filesystem access.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use super::statusline::path::compact_cwd;
use crate::session::{is_cross_project, SessionSummary, Workspace};

/// Sessions of one workspace directory, newest first, plus its display path.
pub(super) struct DirectoryGroup {
    pub(super) cwd: PathBuf,
    pub(super) display: String,
    /// Short name within its repository, such as the worktree folder name.
    /// Equals `display` when the directory is not grouped under a repository.
    pub(super) name: String,
    pub(super) sessions: Vec<SessionSummary>,
}

/// Where a session directory belongs in the hub.
pub(super) enum Placement {
    /// Inside a Git repository; worktrees of one repository share a group.
    Repo(Workspace),
    /// Outside Git, listed as its own group.
    Lone,
    /// The directory no longer exists. Its repository cannot be resolved, so it
    /// joins the shared missing-directories group.
    Missing,
}

/// How a hub group lists its directories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GroupKind {
    /// One directory, sessions listed inline.
    Directory,
    /// Several worktrees of one repository. Only the current directory lists
    /// its sessions inline; the rest collapse to one row each.
    Repo,
    /// Directories that no longer exist, collapsed to one row each.
    Missing,
}

/// Directories shown under one hub section.
pub(super) struct RepoGroup {
    pub(super) display: String,
    pub(super) kind: GroupKind,
    pub(super) directories: Vec<DirectoryGroup>,
}

/// Section title for directories that no longer exist.
const MISSING_SECTION: &str = "MISSING DIRECTORIES";

/// Groups sessions by directory, keeping the input's newest-first order both
/// across groups and inside each group. The current directory always sorts
/// first.
pub(super) fn directory_groups(
    sessions: Vec<SessionSummary>,
    current_cwd: &Path,
) -> Vec<DirectoryGroup> {
    let mut groups: Vec<DirectoryGroup> = Vec::new();
    let mut indexes = HashMap::<PathBuf, usize>::new();
    for session in sessions {
        if let Some(index) = indexes.get(&session.cwd).copied() {
            groups[index].sessions.push(session);
            continue;
        }
        let index = groups.len();
        indexes.insert(session.cwd.clone(), index);
        let display = compact_cwd(&session.cwd);
        groups.push(DirectoryGroup {
            name: display.clone(),
            display,
            cwd: session.cwd.clone(),
            sessions: vec![session],
        });
    }
    if let Some(position) = groups
        .iter()
        .position(|group| !is_cross_project(&group.cwd, current_cwd))
    {
        let current = groups.remove(position);
        groups.insert(0, current);
    }
    groups
}

/// A directory plus its own workspace: group members share a repository but
/// may sit in different worktrees.
type Member = (DirectoryGroup, Option<Workspace>);

#[derive(Clone, PartialEq, Eq, Hash)]
enum GroupKey {
    Repo(PathBuf),
    Lone(PathBuf),
    Missing,
}

/// Groups directories for the hub while keeping [`directory_groups`] order,
/// so the current directory's repository comes first and the current
/// directory leads it. Missing directories collect into one group at the end.
pub(super) fn repo_groups(
    sessions: Vec<SessionSummary>,
    current_cwd: &Path,
    place: impl Fn(&Path) -> Placement,
) -> Vec<RepoGroup> {
    let mut groups: Vec<(GroupKey, Vec<Member>)> = Vec::new();
    let mut indexes = HashMap::<GroupKey, usize>::new();
    for directory in directory_groups(sessions, current_cwd) {
        let (key, workspace) = match place(&directory.cwd) {
            Placement::Repo(workspace) => (GroupKey::Repo(workspace.repo.clone()), Some(workspace)),
            Placement::Lone => (GroupKey::Lone(directory.cwd.clone()), None),
            Placement::Missing => (GroupKey::Missing, None),
        };
        match indexes.get(&key) {
            Some(&index) => groups[index].1.push((directory, workspace)),
            None => {
                indexes.insert(key.clone(), groups.len());
                groups.push((key, vec![(directory, workspace)]));
            }
        }
    }
    // Missing directories are cleanup candidates, not places to work: last.
    groups.sort_by_key(|(key, _)| matches!(key, GroupKey::Missing));
    groups
        .into_iter()
        .map(|(key, members)| {
            let (display, kind) = match &key {
                GroupKey::Missing => (MISSING_SECTION.to_owned(), GroupKind::Missing),
                // A repository with one directory reads best as that directory.
                GroupKey::Repo(repo) if members.len() > 1 => {
                    (compact_cwd(repo_checkout(repo)), GroupKind::Repo)
                }
                GroupKey::Repo(_) | GroupKey::Lone(_) => {
                    (members[0].0.display.clone(), GroupKind::Directory)
                }
            };
            let directories = members
                .into_iter()
                .map(|(mut directory, workspace)| {
                    match (kind, workspace) {
                        (GroupKind::Repo, Some(workspace)) => {
                            directory.name = name_in_worktree(&directory.cwd, &workspace);
                        }
                        // Keep full paths: siblings share no common root.
                        (GroupKind::Missing, _) => directory.name = directory.display.clone(),
                        (GroupKind::Repo | GroupKind::Directory, _) => {}
                    }
                    directory
                })
                .collect();
            RepoGroup {
                display,
                kind,
                directories,
            }
        })
        .collect()
}

/// Folder that holds a repository's `.git` directory, or the Git directory
/// itself for bare repositories.
fn repo_checkout(repo: &Path) -> &Path {
    match (repo.file_name(), repo.parent()) {
        (Some(name), Some(parent)) if name == ".git" => parent,
        _ => repo,
    }
}

/// Worktree folder name, plus the path below it when the directory is nested.
fn name_in_worktree(cwd: &Path, workspace: &Workspace) -> String {
    let worktree = &workspace.worktree;
    let root = worktree.file_name().map_or_else(
        || compact_cwd(worktree),
        |name| name.to_string_lossy().into_owned(),
    );
    match cwd.strip_prefix(worktree) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("{root}/{}", rest.display()),
        _ => root,
    }
}

#[cfg(test)]
#[path = "sessions_hub_groups_tests.rs"]
mod tests;
