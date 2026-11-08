use std::path::Path;

use pretty_assertions::assert_eq;

use super::{
    check_target_in_roots, delete_target_in_roots, delete_targets_in_roots,
    write_linked_run_for_tests, ChildSessionNotDeleted, RunNotFinished,
};
use crate::{
    session::{
        DeleteOptions, Session, SessionInUse, SessionOrigin, SessionProvenance, SessionTarget,
    },
    subagent::RunState,
};

fn create_session(root: &Path, cwd: &Path, id: &str) -> Session {
    Session::create_with_id_in_root(root, cwd, id, None, SessionOrigin::Interactive.into()).unwrap()
}

// Covers: deleting a session deletes the child sessions it started and
// theirs, wherever they were saved, and nothing else; deleting a child keeps
// its parent; a batch that also names a child deleted with its parent
// reports no failure for it; a parent is its id in its own workspace, so a
// same-id session elsewhere keeps its children; a header naming its own
// session as its parent does not stop the delete.
// Owner: session deletion
#[test]
fn delete_takes_child_sessions_recursively() {
    /// Each deleted id with the child sessions deleted along with it.
    type Deleted<'a> = Vec<(&'a str, usize)>;
    // (label, targets, expected deletions, expected sessions gone)
    let cases: [(&str, &[&str], Deleted, Vec<&str>); 5] = [
        (
            "parent",
            &["parent"],
            vec![("parent", 2)],
            vec!["parent", "child", "grandchild"],
        ),
        (
            "child",
            &["child"],
            vec![("child", 1)],
            vec!["child", "grandchild"],
        ),
        (
            "parent and child",
            &["parent", "child"],
            vec![("parent", 2)],
            vec!["parent", "child", "grandchild"],
        ),
        (
            "same id in another workspace",
            &["twin-here"],
            vec![("twin", 1)],
            vec!["twin-here", "twin-here-child"],
        ),
        (
            "own parent",
            &["self-parent"],
            vec![("self-parent", 0)],
            vec!["self-parent"],
        ),
    ];
    for (label, targets, expected_deleted, expected_gone) in cases {
        let sessions = tempfile::tempdir().unwrap();
        let subagents = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        // A subagent can run in another workspace, such as a worktree.
        let elsewhere = tempfile::tempdir().unwrap();
        // (name, id, workspace, parent's name)
        let tree = [
            ("parent", "parent", workspace.path(), None),
            ("child", "child", workspace.path(), Some("parent")),
            ("grandchild", "grandchild", elsewhere.path(), Some("child")),
            ("other-root", "other-root", workspace.path(), None),
            ("sibling", "sibling", workspace.path(), Some("other-root")),
            // One id saved in two workspaces, each with a child here.
            ("twin-here", "twin", workspace.path(), None),
            (
                "twin-here-child",
                "twin-here-child",
                workspace.path(),
                Some("twin-here"),
            ),
            ("twin-elsewhere", "twin", elsewhere.path(), None),
            (
                "twin-elsewhere-child",
                "twin-elsewhere-child",
                workspace.path(),
                Some("twin-elsewhere"),
            ),
            (
                "self-parent",
                "self-parent",
                workspace.path(),
                Some("self-parent"),
            ),
        ];
        let target = |name: &str| {
            let (_, id, cwd, _) = tree.iter().find(|(entry, ..)| *entry == name).unwrap();
            SessionTarget::new(*id, *cwd)
        };
        let paths = tree.map(|(name, id, cwd, parent)| {
            let provenance = match parent {
                Some(parent) => SessionProvenance {
                    origin: SessionOrigin::Subagent,
                    parent: Some(target(parent)),
                },
                None => SessionOrigin::Interactive.into(),
            };
            let session =
                Session::create_with_id_in_root(sessions.path(), cwd, id, None, provenance)
                    .unwrap();
            (name, session.path().to_path_buf())
        });
        let targets = targets.iter().copied().map(target).collect::<Vec<_>>();

        let outcome = delete_targets_in_roots(
            sessions.path(),
            subagents.path(),
            &targets,
            &DeleteOptions::default(),
        )
        .unwrap();

        let deleted = outcome
            .deleted
            .iter()
            .map(|item| (item.id.as_str(), item.deleted_child_session_count))
            .collect::<Vec<_>>();
        let failed = outcome
            .failures
            .iter()
            .map(|item| item.error.as_str())
            .collect::<Vec<_>>();
        let gone = paths
            .iter()
            .filter(|(_, path)| !path.exists())
            .map(|(name, _)| *name)
            .collect::<Vec<_>>();
        assert_eq!(
            (deleted, failed, gone),
            (expected_deleted, Vec::<&str>::new(), expected_gone),
            "{label}"
        );
    }
}

// Covers: a child session that refuses its delete stops its parent's: the
// parent and the children after the refused one stay, the error names the
// child and its parent, and the children deleted before it leave the index
// even though the delete failed.
// Owner: session deletion
#[test]
fn refused_child_session_keeps_its_parent() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let parent = SessionTarget::new("parent", workspace.path());
    // Children are deleted oldest first, so in this order.
    let ids = ["parent", "child-a", "child-b", "child-c"];
    let paths = ids.map(|id| {
        let provenance = match id {
            "parent" => SessionOrigin::Interactive.into(),
            _ => SessionProvenance {
                origin: SessionOrigin::Subagent,
                parent: Some(parent.clone()),
            },
        };
        Session::create_with_id_in_root(sessions.path(), workspace.path(), id, None, provenance)
            .unwrap()
            .path()
            .to_path_buf()
    });
    let refused = SessionTarget::new("child-b", workspace.path());

    let error = delete_target_in_roots(
        sessions.path(),
        subagents.path(),
        &parent,
        &DeleteOptions {
            force: false,
            protected_session: Some(refused.clone()),
        },
    )
    .unwrap_err();

    let named = error
        .downcast_ref::<ChildSessionNotDeleted>()
        .map(|error| (error.child.clone(), error.parent.clone()));
    let remaining = ids
        .iter()
        .zip(&paths)
        .filter(|(_, path)| path.exists())
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    // Read the index directly: listing would reconcile leftover rows away.
    let index = rusqlite::Connection::open(sessions.path().join("index.sqlite3")).unwrap();
    let indexed = index
        .prepare("select id from sessions order by id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        (named, remaining, indexed),
        (
            Some((refused, parent)),
            vec!["parent", "child-b", "child-c"],
            vec!["child-b".to_string(), "child-c".into(), "parent".into()],
        )
    );
}

// Covers: the check a host runs before it lets an open session go to
// delete it refuses as the delete would, for a child session something
// holds or a child's unfinished run, but deletes nothing, not even the
// children before the refused one that the delete itself would take; it
// skips the session's own lease, which that host holds (#1670).
// Owner: session deletion
#[test]
fn delete_check_refuses_as_the_delete_would_and_deletes_nothing() {
    /// What refused the check: the child it names and what that child's
    /// error is.
    #[derive(Debug, PartialEq, Eq)]
    enum Refusal {
        ChildInUse(String),
        ChildRunNotFinished(String),
    }
    // (label, child session held open, child whose run is unfinished, expected)
    let cases = [
        ("clear", None, None, Ok(())),
        (
            "child held",
            Some("child-b"),
            None,
            Err(Refusal::ChildInUse("child-b".into())),
        ),
        (
            "child's run unfinished",
            None,
            Some("child-b"),
            Err(Refusal::ChildRunNotFinished("child-b".into())),
        ),
    ];
    for (label, held_child, running_child, expected) in cases {
        let sessions = tempfile::tempdir().unwrap();
        let subagents = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let parent = SessionTarget::new("parent", workspace.path());
        let ids = ["parent", "child-a", "child-b"];
        let paths = ids.map(|id| {
            let provenance = match id {
                "parent" => SessionOrigin::Interactive.into(),
                _ => SessionProvenance {
                    origin: SessionOrigin::Subagent,
                    parent: Some(parent.clone()),
                },
            };
            Session::create_with_id_in_root(sessions.path(), workspace.path(), id, None, provenance)
                .unwrap()
                .path()
                .to_path_buf()
        });
        // The host's own hold on the session it deletes.
        let _host =
            Session::open_by_id_in_root(sessions.path(), workspace.path(), "parent").unwrap();
        let _held = held_child
            .map(|id| Session::open_by_id_in_root(sessions.path(), workspace.path(), id).unwrap());
        if let Some(id) = running_child {
            write_linked_run_for_tests(subagents.path(), "ee33ff", id, RunState::Running);
        }

        let checked = check_target_in_roots(
            sessions.path(),
            subagents.path(),
            &parent,
            &DeleteOptions::default(),
        )
        .map_err(|error| {
            let child = error
                .downcast_ref::<ChildSessionNotDeleted>()
                .map(|refused| refused.child.id.clone())
                .unwrap_or_default();
            if error.chain().any(|cause| cause.is::<SessionInUse>()) {
                Refusal::ChildInUse(child)
            } else if error.chain().any(|cause| cause.is::<RunNotFinished>()) {
                Refusal::ChildRunNotFinished(child)
            } else {
                panic!("{label}: unexpected refusal: {error:#}")
            }
        });

        let remaining = ids
            .iter()
            .zip(&paths)
            .filter(|(_, path)| path.exists())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        assert_eq!((checked, remaining), (expected, ids.to_vec()), "{label}");
    }
}

// Covers: an exact workspace target cannot fall back to a same-id session in
// another workspace during delete.
// Owner: session deletion
#[test]
fn exact_target_delete_preserves_same_id_in_other_workspace() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace_a = tempfile::tempdir().unwrap();
    let workspace_b = tempfile::tempdir().unwrap();
    let id = "00000000-0000-0000-0000-000000000123";
    let kept = create_session(sessions.path(), workspace_a.path(), id);
    let deleted = create_session(sessions.path(), workspace_b.path(), id);
    let deleted_path = deleted.path().to_path_buf();
    drop(deleted);
    let outcome = delete_target_in_roots(
        sessions.path(),
        subagents.path(),
        &SessionTarget::new(id, workspace_b.path()),
        &DeleteOptions::default(),
    )
    .unwrap();

    assert_eq!(outcome.cwd, workspace_b.path());
    assert!(kept.path().exists());
    assert!(!deleted_path.exists());
}

// Covers: workspace batch deletion owns current-session protection and deletes
// every other persisted session.
// Owner: session deletion
#[test]
fn workspace_delete_keeps_exact_protected_session() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let kept_id = "00000000-0000-0000-0000-000000000124";
    let deleted_id = "00000000-0000-0000-0000-000000000125";
    let kept = create_session(sessions.path(), workspace.path(), kept_id);
    let deleted = create_session(sessions.path(), workspace.path(), deleted_id);
    let deleted_path = deleted.path().to_path_buf();
    drop(deleted);
    let protected = SessionTarget::new(kept_id, workspace.path());

    let outcome = delete_targets_in_roots(
        sessions.path(),
        subagents.path(),
        &[
            protected.clone(),
            SessionTarget::new(deleted_id, workspace.path()),
        ],
        &DeleteOptions {
            force: false,
            protected_session: Some(protected.clone()),
        },
    )
    .unwrap();

    assert_eq!(outcome.kept_protected, vec![protected]);
    assert_eq!(
        outcome
            .deleted
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec![deleted_id]
    );
    assert!(outcome.failures.is_empty());
    assert!(kept.path().exists());
    assert!(!deleted_path.exists());
}

// Covers: a child spawned while the session was open keeps a copy of the
// lease handle until exec, so closing the session must release the lease itself.
// Owner: session deletion
#[cfg(unix)]
#[test]
fn closed_session_is_deletable_while_an_inherited_lease_handle_is_open() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let id = "00000000-0000-0000-0000-000000000127";
    let session = create_session(sessions.path(), workspace.path(), id);
    let inherited = session
        .active_lease_for_tests()
        .inherited_handle_for_tests();
    let path = session.path().to_path_buf();
    drop(session);

    delete_target_in_roots(
        sessions.path(),
        subagents.path(),
        &SessionTarget::new(id, workspace.path()),
        &DeleteOptions::default(),
    )
    .unwrap();
    assert!(!path.exists());
    drop(inherited);
}

// Covers: batch deletion removes only targets reviewed before confirmation, so
// a session created between preview and confirmation survives.
// Owner: session deletion
#[test]
fn target_batch_does_not_delete_session_created_after_preview() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let reviewed_id = "00000000-0000-0000-0000-000000000127";
    let later_id = "00000000-0000-0000-0000-000000000128";
    let reviewed = create_session(sessions.path(), workspace.path(), reviewed_id);
    let reviewed_path = reviewed.path().to_path_buf();
    let reviewed_targets = vec![SessionTarget::new(reviewed_id, workspace.path())];
    drop(reviewed);

    let later = create_session(sessions.path(), workspace.path(), later_id);
    let later_path = later.path().to_path_buf();
    drop(later);

    let outcome = delete_targets_in_roots(
        sessions.path(),
        subagents.path(),
        &reviewed_targets,
        &DeleteOptions::default(),
    )
    .unwrap();

    assert_eq!(outcome.deleted.len(), 1);
    assert!(!reviewed_path.exists());
    assert!(later_path.exists());
}

// Covers: a batch keeps going past a session it cannot delete, and drops the
// index rows of the ones it did delete even though that happens at batch end.
// Owner: session deletion
#[test]
fn batch_delete_skips_blocked_session_and_drops_deleted_index_rows() {
    let sessions = tempfile::tempdir().unwrap();
    let subagents = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let ids = [
        "00000000-0000-0000-0000-000000000131",
        "00000000-0000-0000-0000-000000000132",
        "00000000-0000-0000-0000-000000000133",
    ];
    for id in &ids {
        drop(create_session(sessions.path(), workspace.path(), id));
    }
    // An open session holds its active lease, which blocks its delete.
    let blocked = Session::open_by_id_in_root(sessions.path(), workspace.path(), ids[1])
        .unwrap()
        .0;

    let outcome = delete_targets_in_roots(
        sessions.path(),
        subagents.path(),
        &ids.map(|id| SessionTarget::new(id, workspace.path())),
        &DeleteOptions::default(),
    )
    .unwrap();

    let deleted = outcome
        .deleted
        .iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>();
    let failed = outcome
        .failures
        .iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!((deleted, failed), (vec![ids[0], ids[2]], vec![ids[1]]));
    drop(blocked);
    // Read the index directly: listing would reconcile leftover rows away.
    let index = rusqlite::Connection::open(sessions.path().join("index.sqlite3")).unwrap();
    let indexed = index
        .prepare("select id from sessions order by id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(indexed, vec![ids[1].to_string()]);
}
