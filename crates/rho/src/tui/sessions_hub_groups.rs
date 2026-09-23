//! `/sessions` hub grouping: sessions by directory, directories by repository.
//!
//! Worktrees of one Git repository share a [`HubGroup::Repo`] so a repo with
//! many checkouts reads as one block instead of one section per checkout.
//! Deleted directories cannot name their repository, so they collect in one
//! [`HubGroup::Missing`] at the end. Placement comes from an injected resolver,
//! keeping this module free of filesystem access.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use super::statusline::path::compact_cwd;
use crate::session::{is_cross_project, SessionSummary, Workspace};

/// Sessions of one workspace directory, newest first, plus its display path.
#[derive(Debug)]
pub(super) struct DirectoryGroup {
    pub(super) cwd: PathBuf,
    pub(super) display: String,
    pub(super) sessions: Vec<SessionSummary>,
}

/// One directory inside a multi-worktree repository.
#[derive(Debug)]
pub(super) struct Worktree {
    /// Worktree folder name, plus the subpath for nested directories.
    pub(super) name: String,
    pub(super) directory: DirectoryGroup,
}

/// One hub section.
#[derive(Debug)]
pub(super) enum HubGroup {
    /// A single directory: a non-Git directory, or a repository with only one
    /// directory holding sessions.
    Directory(DirectoryGroup),
    /// Several directories sharing one Git repository.
    Repo {
        display: String,
        worktrees: Vec<Worktree>,
    },
    /// Directories that no longer exist.
    Missing(Vec<DirectoryGroup>),
}

/// The group for directory `cwd`, wherever it sits in the hub.
pub(super) fn find_directory<'a>(groups: &'a [HubGroup], cwd: &Path) -> Option<&'a DirectoryGroup> {
    groups.iter().find_map(|group| match group {
        HubGroup::Directory(directory) => (directory.cwd == cwd).then_some(directory),
        HubGroup::Repo { worktrees, .. } => worktrees
            .iter()
            .map(|worktree| &worktree.directory)
            .find(|directory| directory.cwd == cwd),
        HubGroup::Missing(directories) => directories.iter().find(|directory| directory.cwd == cwd),
    })
}

/// Where a session directory belongs in the hub.
pub(super) enum Placement {
    /// Inside a Git repository; worktrees of one repository share a group.
    Repo(Workspace),
    /// Outside Git, listed as its own group.
    Lone,
    /// The directory no longer exists.
    Missing,
}

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
        groups.push(DirectoryGroup {
            display: compact_cwd(&session.cwd),
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

/// One hub section in first-seen order. Repositories point into a separate
/// member list so later worktrees can join a section already placed.
enum Slot {
    Directory(DirectoryGroup),
    Repo(usize),
}

/// Groups directories into hub sections while keeping [`directory_groups`]
/// order, so the current directory's section comes first and the current
/// directory leads it. Missing directories collect into one final group.
pub(super) fn hub_groups(
    sessions: Vec<SessionSummary>,
    current_cwd: &Path,
    place: impl Fn(&Path) -> Placement,
) -> Vec<HubGroup> {
    let mut slots = Vec::new();
    // Members keep their own workspace: they share a repository but may sit
    // in different worktrees.
    let mut repos: Vec<Vec<(DirectoryGroup, Workspace)>> = Vec::new();
    let mut repo_indexes = HashMap::<PathBuf, usize>::new();
    let mut missing = Vec::new();
    for directory in directory_groups(sessions, current_cwd) {
        match place(&directory.cwd) {
            Placement::Missing => missing.push(directory),
            Placement::Lone => slots.push(Slot::Directory(directory)),
            Placement::Repo(workspace) => {
                let index = *repo_indexes
                    .entry(workspace.repo.clone())
                    .or_insert_with(|| {
                        slots.push(Slot::Repo(repos.len()));
                        repos.push(Vec::new());
                        repos.len() - 1
                    });
                repos[index].push((directory, workspace));
            }
        }
    }
    let mut groups = slots
        .into_iter()
        .map(|slot| match slot {
            Slot::Directory(directory) => HubGroup::Directory(directory),
            Slot::Repo(index) => repo_group(std::mem::take(&mut repos[index])),
        })
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        groups.push(HubGroup::Missing(missing));
    }
    groups
}

/// A repository whose sessions all sit in one directory reads best as that
/// directory.
fn repo_group(mut members: Vec<(DirectoryGroup, Workspace)>) -> HubGroup {
    if members.len() == 1 {
        return HubGroup::Directory(members.remove(0).0);
    }
    HubGroup::Repo {
        display: compact_cwd(repo_checkout(&members[0].1.repo)),
        worktrees: members
            .into_iter()
            .map(|(directory, workspace)| Worktree {
                name: name_in_worktree(&directory.cwd, &workspace.worktree),
                directory,
            })
            .collect(),
    }
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
fn name_in_worktree(cwd: &Path, worktree: &Path) -> String {
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
