use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{
    directory_picker, hub_picker, DirectoryGroup, GroupKind, RepoGroup, SessionsHubTarget,
};
use crate::session::{SessionSummary, SessionTarget};

fn summary(id: &str, cwd: &str, updated_at: u64) -> SessionSummary {
    SessionSummary {
        id: id.to_string(),
        path: PathBuf::from(format!("/sessions/{id}")),
        cwd: PathBuf::from(cwd),
        created_at: updated_at,
        updated_at,
        message_count: 2,
        title: Some(format!("title {id}")),
        first_user_message: Some(format!("first {id}")),
        last_user_message: Some(format!("last {id}")),
    }
}

fn group(cwd: &str, display: &str, sessions: Vec<SessionSummary>) -> DirectoryGroup {
    DirectoryGroup {
        cwd: PathBuf::from(cwd),
        display: display.to_string(),
        name: display.to_string(),
        sessions,
    }
}

fn lone(directory: DirectoryGroup) -> RepoGroup {
    RepoGroup {
        display: directory.display.clone(),
        kind: GroupKind::Directory,
        directories: vec![directory],
    }
}

// Covers: picker rows retain exact workspace identity even when two workspaces
// use the same session id.
// Owner: sessions hub picker state
#[test]
fn hub_picker_builds_typed_workspace_targets() {
    let groups = vec![
        lone(group(
            "/work/current",
            "~/current",
            vec![summary("same-session", "/work/current", 200)],
        )),
        lone(group(
            "/work/other",
            "~/other",
            vec![summary("same-session", "/work/other", 100)],
        )),
    ];
    let current = SessionTarget::new("same-session", "/work/current");

    let build = hub_picker(
        &groups,
        Some(&current),
        Path::new("/work/current"),
        1_000,
        Some((2, 1)),
    );

    assert_eq!(
        build.targets,
        vec![
            SessionsHubTarget::CleanupMissingWorkspaces,
            SessionsHubTarget::Directory(PathBuf::from("/work/current")),
            SessionsHubTarget::Session(SessionTarget::new("same-session", "/work/current")),
            SessionsHubTarget::Directory(PathBuf::from("/work/other")),
            SessionsHubTarget::Session(SessionTarget::new("same-session", "/work/other")),
        ]
    );
}

// Covers: the drill-in list keeps session rows only, without section headers,
// so escape-back and scoped delete operate on one directory.
// Owner: sessions hub picker
#[test]
fn directory_picker_lists_only_that_directorys_sessions() {
    let scoped = group(
        "/work/other",
        "~/other",
        vec![
            summary("a-session", "/work/other", 200),
            summary("b-session", "/work/other", 100),
        ],
    );

    let build = directory_picker(&scoped, None, Path::new("/work/current"), 1_000);

    assert_eq!(build.picker.title, "~/other");
    assert!(build.picker.items.iter().all(|item| item.section.is_none()));
    assert_eq!(
        build.targets,
        vec![
            SessionsHubTarget::Session(SessionTarget::new("a-session", "/work/other")),
            SessionsHubTarget::Session(SessionTarget::new("b-session", "/work/other")),
        ]
    );
}

// Covers: inside a multi-worktree repo, only the current worktree lists its
// sessions inline; sibling worktrees collapse to one browsable row each.
// Owner: sessions hub picker
#[test]
fn hub_picker_collapses_sibling_worktrees() {
    let mut current = group(
        "/repo/wt-a",
        "/repo/wt-a",
        vec![summary("a-session", "/repo/wt-a", 200)],
    );
    current.name = "wt-a".into();
    let mut sibling = group(
        "/repo/wt-b",
        "/repo/wt-b",
        vec![summary("b-session", "/repo/wt-b", 100)],
    );
    sibling.name = "wt-b".into();
    let repos = vec![RepoGroup {
        display: "/repo".into(),
        kind: GroupKind::Repo,
        directories: vec![current, sibling],
    }];

    let build = hub_picker(&repos, None, Path::new("/repo/wt-a"), 1_000, None);

    assert_eq!(
        build.targets,
        vec![
            SessionsHubTarget::Directory(PathBuf::from("/repo/wt-a")),
            SessionsHubTarget::Session(SessionTarget::new("a-session", "/repo/wt-a")),
            SessionsHubTarget::Directory(PathBuf::from("/repo/wt-b")),
        ]
    );
    let rows = build
        .picker
        .items
        .iter()
        .map(|item| (item.section.as_deref(), item.label.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            (Some("/repo"), "wt-a · 1"),
            (Some("/repo"), "title a-session"),
            (Some("/repo"), "wt-b · 1"),
        ]
    );
}
