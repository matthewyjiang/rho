use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{directory_groups, hub_groups, HubGroup, Placement};
use crate::session::{SessionSummary, Workspace};

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

/// Fake placement: `/repo/<worktree>/...` belongs to `/repo/.git`, `/solo`
/// is its own one-worktree repo, `/gone/...` is deleted, and anything else is
/// outside Git.
fn place(cwd: &Path) -> Placement {
    if cwd.starts_with("/gone") {
        return Placement::Missing;
    }
    if let Ok(rest) = cwd.strip_prefix("/repo") {
        let worktree = rest.components().next().expect("worktree folder");
        return Placement::Repo(Workspace {
            worktree: Path::new("/repo").join(worktree),
            repo: PathBuf::from("/repo/.git"),
        });
    }
    if cwd == Path::new("/solo") {
        return Placement::Repo(Workspace {
            worktree: PathBuf::from("/solo"),
            repo: PathBuf::from("/solo/.git"),
        });
    }
    Placement::Lone
}

// Covers: worktrees of one repo merge under the repo's checkout name with
// worktree-relative row names, the current directory's repo leads, single
// directories keep their full path, and missing directories collect last.
// Owner: sessions hub grouping
#[test]
fn hub_groups_merge_worktrees_and_collect_missing_directories() {
    let sessions = vec![
        summary("gone-a", "/gone/wt-x", 700),
        summary("plain", "/plain", 600),
        summary("solo", "/solo", 500),
        summary("b", "/repo/wt-b", 400),
        summary("gone-b", "/gone/wt-y", 350),
        summary("a-nested", "/repo/wt-a/crates/x", 300),
        summary("a", "/repo/wt-a", 200),
    ];

    let groups = hub_groups(sessions, Path::new("/repo/wt-a"), place);

    let names = |directories: &[super::DirectoryGroup]| {
        directories
            .iter()
            .map(|directory| directory.display.clone())
            .collect::<Vec<_>>()
    };
    let shape = groups
        .iter()
        .map(|group| match group {
            HubGroup::Directory(directory) => ("directory", directory.display.clone(), vec![]),
            HubGroup::Repo { display, worktrees } => (
                "repo",
                display.clone(),
                worktrees
                    .iter()
                    .map(|worktree| worktree.name.clone())
                    .collect(),
            ),
            HubGroup::Missing(missing) => ("missing", String::new(), names(missing)),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shape,
        vec![
            (
                "repo",
                "/repo".to_string(),
                vec![
                    "wt-a".to_string(),
                    "wt-b".to_string(),
                    "wt-a/crates/x".to_string()
                ]
            ),
            ("directory", "/plain".to_string(), vec![]),
            ("directory", "/solo".to_string(), vec![]),
            (
                "missing",
                String::new(),
                vec!["/gone/wt-x".to_string(), "/gone/wt-y".to_string()]
            ),
        ]
    );
}
