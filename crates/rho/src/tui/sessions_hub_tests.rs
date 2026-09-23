use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{directory_picker, hub_picker, DirectoryGroup, HubGroup, SessionsHubTarget};
use crate::session::{SessionSummary, SessionTarget};
use crate::tui::sessions_hub_groups::Worktree;

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
        sessions,
    }
}

// Covers: picker rows retain exact workspace identity even when two workspaces
// use the same session id, and missing directories never inline sessions.
// Owner: sessions hub picker state
#[test]
fn hub_picker_builds_typed_workspace_targets() {
    let groups = vec![
        HubGroup::Directory(group(
            "/work/current",
            "~/current",
            vec![summary("same-session", "/work/current", 200)],
        )),
        HubGroup::Directory(group(
            "/work/other",
            "~/other",
            vec![summary("same-session", "/work/other", 100)],
        )),
        HubGroup::Missing(vec![group(
            "/gone",
            "/gone",
            vec![summary("same-session", "/gone", 50)],
        )]),
    ];
    let current = SessionTarget::new("same-session", "/work/current");

    let build = hub_picker(&groups, Some(&current), Path::new("/work/current"), 1_000);

    // Missing directories get the leading cleanup row and a directory row,
    // but never list their sessions inline.
    assert_eq!(
        build.targets,
        vec![
            SessionsHubTarget::CleanupMissingWorkspaces,
            SessionsHubTarget::Directory(PathBuf::from("/work/current")),
            SessionsHubTarget::Session(SessionTarget::new("same-session", "/work/current")),
            SessionsHubTarget::Directory(PathBuf::from("/work/other")),
            SessionsHubTarget::Session(SessionTarget::new("same-session", "/work/other")),
            SessionsHubTarget::Directory(PathBuf::from("/gone")),
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
    let worktree = |name: &str, session: &str| Worktree {
        name: name.into(),
        directory: group(
            &format!("/repo/{name}"),
            &format!("/repo/{name}"),
            vec![summary(session, &format!("/repo/{name}"), 100)],
        ),
    };
    let groups = vec![HubGroup::Repo {
        display: "/repo".into(),
        worktrees: vec![worktree("wt-a", "a-session"), worktree("wt-b", "b-session")],
    }];

    let build = hub_picker(&groups, None, Path::new("/repo/wt-a"), 1_000);

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
