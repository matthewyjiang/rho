use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{directory_groups, directory_picker, hub_picker, DirectoryGroup, SessionsHubTarget};
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
        sessions,
    }
}

// Covers: the hub lists the current directory first while other directories
// keep newest-first order, which decides what the picker opens on.
// Owner: sessions hub grouping
#[test]
fn directory_groups_put_the_current_directory_first() {
    let sessions = vec![
        summary("s-newest", "/work/other", 300),
        summary("s-current", "/work/current", 200),
        summary("s-older", "/work/other", 100),
        summary("s-third", "/work/third", 50),
    ];

    let groups = directory_groups(sessions, Path::new("/work/current"));

    let cwds = groups
        .iter()
        .map(|group| group.cwd.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        cwds,
        vec![
            PathBuf::from("/work/current"),
            PathBuf::from("/work/other"),
            PathBuf::from("/work/third"),
        ]
    );
    assert_eq!(
        groups[1]
            .sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        vec!["s-newest", "s-older"]
    );
}

// Covers: picker rows retain exact workspace identity even when two workspaces
// use the same session id.
// Owner: sessions hub picker state
#[test]
fn hub_picker_builds_typed_workspace_targets() {
    let groups = vec![
        group(
            "/work/current",
            "~/current",
            vec![summary("same-session", "/work/current", 200)],
        ),
        group(
            "/work/other",
            "~/other",
            vec![summary("same-session", "/work/other", 100)],
        ),
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
