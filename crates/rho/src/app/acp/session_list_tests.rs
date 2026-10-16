use std::path::{Path, PathBuf};

use agent_client_protocol::{
    schema::v1::{SessionId, SessionInfo},
    ErrorCode,
};
use pretty_assertions::assert_eq;
use rho_sdk::model::Message;

use super::{page, PAGE_SIZE};
use crate::session::{Session as StoredSession, SessionSummary};

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
    }
}

// Covers: a host that follows nextCursor, as Zed's thread import does, must
// see every resumable session exactly once, newest first, even when a page
// boundary splits sessions saved in the same second, or one id saved in two
// workspaces; empty sessions and sessions in deleted workspaces must not be
// offered.
// Owner: ACP session/list paging
#[test]
fn following_cursors_lists_each_resumable_session_once() {
    let root = tempfile::tempdir().unwrap();
    let (first, second) = (root.path().join("a"), root.path().join("b"));
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let gone = root.path().join("deleted-workspace");
    let mut saved = (0..PAGE_SIZE * 2 + 1)
        .map(|n| summary(&format!("{n:04}"), &first, 10_000 - n as u64 / 3, 2))
        .collect::<Vec<_>>();
    // The last entry of page one, saved again in the second workspace, so
    // the first page ends between the two.
    let twin = summary(
        &saved[PAGE_SIZE - 1].id,
        &second,
        saved[PAGE_SIZE - 1].updated_at,
        2,
    );
    saved.insert(PAGE_SIZE, twin);
    let mut store = saved.clone();
    store.push(summary("empty", &first, 20_000, 0));
    store.push(summary("orphan", &gone, 20_000, 2));
    store.reverse();

    let mut listed = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let response = page(store.clone(), cursor.as_deref()).unwrap();
        pages += 1;
        listed.extend(
            response
                .sessions
                .into_iter()
                .map(|info| (info.session_id, info.cwd)),
        );
        match response.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    let expected = saved
        .iter()
        .map(|summary| (SessionId::new(summary.id.as_str()), summary.cwd.clone()))
        .collect::<Vec<_>>();
    assert_eq!((pages, listed), (3, expected));
}

// Covers: a cursor Rho did not issue must fail as invalid params rather than
// restart the walk at the first page, which would loop a paging host.
// Owner: ACP session/list paging
#[test]
fn foreign_cursor_is_invalid_params() {
    for cursor in ["", "page-2", "later/0001", "[1, \"0001\"]"] {
        let error = page(Vec::new(), Some(cursor)).expect_err(cursor);
        assert_eq!(error.code, ErrorCode::InvalidParams, "{cursor}");
    }
}

// Covers: listed entries carry the label /resume shows (the title, else the
// first user message on one line) and an RFC 3339 updatedAt, the format Zed
// parses; an unparseable date would drop the entry's recency.
// Owner: ACP session/list mapping
#[test]
fn entries_carry_resume_label_and_rfc3339_time() {
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path();
    let long = "word ".repeat(40);
    let cases = [
        (
            Some("Fix login"),
            Some("ignored"),
            Some("Fix login".to_owned()),
        ),
        (
            None,
            Some("  fix the\n  login bug \n"),
            Some("fix the login bug".to_owned()),
        ),
        (None, Some(long.as_str()), Some(format!("{}…", &long[..79]))),
        (None, Some(" \n "), None),
        (None, None, None),
    ];
    for (title, first_user_message, expected) in cases {
        let mut stored = summary("one", cwd, 1_792_133_984, 2);
        stored.title = title.map(str::to_owned);
        stored.first_user_message = first_user_message.map(str::to_owned);

        let listed = page(vec![stored], None).unwrap().sessions;

        assert_eq!(
            listed,
            vec![SessionInfo::new(SessionId::new("one"), cwd)
                .title(expected)
                .updated_at("2026-10-16T06:59:44Z".to_owned())]
        );
    }
}

// Covers: the ids session/list returns from a real store must be the ids
// session/load opens, and a session with nothing saved stays hidden as in
// /resume.
// Owner: ACP session/list against the session store
#[test]
fn listed_store_sessions_open_by_their_listed_id() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path().canonicalize().unwrap();
    let saved = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    saved
        .append_message(&Message::user_text("list me"))
        .unwrap();
    let saved_id = saved.id().to_owned();
    drop(saved);
    drop(StoredSession::create_in_root(root.path(), &cwd).unwrap());

    let store = StoredSession::list_in_root_for_test(root.path(), &cwd).unwrap();
    let listed = page(store, None).unwrap().sessions;

    assert_eq!(
        listed
            .iter()
            .map(|info| (info.session_id.to_string(), info.cwd.clone()))
            .collect::<Vec<_>>(),
        vec![(saved_id.clone(), PathBuf::from(&cwd))]
    );
    let (opened, histories) = StoredSession::open_by_id_with_histories_in_root(
        root.path(),
        &listed[0].cwd,
        &listed[0].session_id.to_string(),
    )
    .unwrap();
    assert_eq!(
        (opened.id(), histories.display),
        (saved_id.as_str(), vec![Message::user_text("list me")])
    );
}
