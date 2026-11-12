use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rho_sdk::model::Message;

use super::{list_in_root, page, Cursor, DEFAULT_PAGE_SIZE};
use crate::session::{layout::session_dir_in_root, Session as StoredSession, SessionSummary};

fn summary(id: &str, cwd: &Path, updated_at: u64, message_count: u64) -> SessionSummary {
    SessionSummary {
        id: id.to_owned(),
        path: cwd.join(format!("{id}.jsonl")),
        cwd: cwd.to_path_buf(),
        created_at: 0,
        updated_at,
        message_count,
        title: Some(format!("title {id}")),
        first_user_message: None,
        last_user_message: None,
        added_dirs: Vec::new(),
        origin: None,
    }
}

/// A session saved in `cwd` under `root` with one user message, so it is
/// listed; its id.
fn saved_session(root: &Path, cwd: &Path) -> String {
    let saved = StoredSession::create_in_root(root, cwd).unwrap();
    saved
        .append_message(&Message::user_text("list me"))
        .unwrap();
    saved.id().to_owned()
}

// Covers: a client that follows next_cursor, as Zed's thread import does
// through ACP's session/list, or any daemon client through
// rho/session/list, must see every resumable session exactly once, newest
// first, even when a page boundary splits sessions saved in the same
// second, or one id saved in two workspaces; empty sessions and sessions in
// deleted workspaces must not be offered.
// Owner: session catalog paging
#[test]
fn following_cursors_lists_each_resumable_session_once() {
    let size = DEFAULT_PAGE_SIZE.get();
    let root = tempfile::tempdir().unwrap();
    let (first, second) = (root.path().join("a"), root.path().join("b"));
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let gone = root.path().join("deleted-workspace");
    let mut saved = (0..size * 2 + 1)
        .map(|n| summary(&format!("{n:04}"), &first, 10_000 - n as u64 / 3, 2))
        .collect::<Vec<_>>();
    // The last entry of page one, saved again in the second workspace, so
    // the first page ends between the two.
    let twin = summary(&saved[size - 1].id, &second, saved[size - 1].updated_at, 2);
    saved.insert(size, twin);
    let mut store = saved.clone();
    store.push(summary("empty", &first, 20_000, 0));
    store.push(summary("orphan", &gone, 20_000, 2));
    store.reverse();

    let mut listed = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let after = cursor
            .as_deref()
            .map(|cursor| Cursor::parse(cursor).unwrap());
        let next = page(store.clone(), after.as_ref(), DEFAULT_PAGE_SIZE);
        pages += 1;
        listed.extend(
            next.sessions
                .into_iter()
                .map(|summary| (summary.id, summary.cwd)),
        );
        match next.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    let expected = saved
        .iter()
        .map(|summary| (summary.id.clone(), summary.cwd.clone()))
        .collect::<Vec<_>>();
    assert_eq!((pages, listed), (3, expected));
}

// Covers: a cursor Rho did not issue must fail rather than restart the walk
// at the first page, which would loop a paging client.
// Owner: session catalog paging
#[test]
fn a_foreign_cursor_is_unknown() {
    for cursor in ["", "page-2", "later/0001", "[1, \"0001\"]"] {
        assert_eq!(Cursor::parse(cursor), None, "{cursor}");
    }
}

// Covers: the ids the catalog lists from a real store must be the ids that
// open, and a session with nothing saved stays hidden as in /resume.
// Owner: session catalog against the session store
#[test]
fn listed_store_sessions_open_by_their_listed_id() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    // As a session names its workspace, which is how the catalog looks it up.
    let cwd = crate::paths::canonicalize(workspace.path()).unwrap();
    let saved_id = saved_session(root.path(), &cwd);
    drop(StoredSession::create_in_root(root.path(), &cwd).unwrap());

    let listed = list_in_root(root.path(), Some(&cwd), None, DEFAULT_PAGE_SIZE)
        .unwrap()
        .sessions;

    assert_eq!(
        listed
            .iter()
            .map(|summary| (summary.id.clone(), summary.cwd.clone()))
            .collect::<Vec<_>>(),
        vec![(saved_id.clone(), PathBuf::from(&cwd))]
    );
    let (opened, histories) = StoredSession::open_by_id_with_histories_in_root(
        root.path(),
        &listed[0].cwd,
        &listed[0].id,
    )
    .unwrap();
    assert_eq!(
        (opened.id(), histories.display),
        (saved_id.as_str(), vec![Message::user_text("list me")])
    );
}

// Covers: a workspace named through a symlink lists the sessions saved in
// the directory it resolves to, as `rho/session/new` canonicalizes the
// workspace it saves under; a path that resolves nowhere lists none and
// leaves no session directory behind.
// Owner: session catalog against the session store
#[cfg(unix)]
#[test]
fn a_workspace_lists_by_its_canonical_path() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path().canonicalize().unwrap();
    let saved_id = saved_session(root.path(), &cwd);
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("workspace-link");
    std::os::unix::fs::symlink(&cwd, &link).unwrap();
    let missing = links.path().join("missing");

    let through_link = list_in_root(root.path(), Some(&link), None, DEFAULT_PAGE_SIZE)
        .unwrap()
        .sessions
        .into_iter()
        .map(|summary| (summary.id, summary.cwd))
        .collect::<Vec<_>>();
    let nowhere = list_in_root(root.path(), Some(&missing), None, DEFAULT_PAGE_SIZE).unwrap();

    assert_eq!(
        (
            through_link,
            nowhere.sessions,
            nowhere.next_cursor,
            session_dir_in_root(root.path(), &missing).exists()
        ),
        (vec![(saved_id, cwd)], Vec::new(), None, false)
    );
}
