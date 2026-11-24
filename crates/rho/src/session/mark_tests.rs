use std::{fs, path::Path};

use pretty_assertions::assert_eq;
use rusqlite::params;
use tempfile::TempDir;

use super::super::{
    persistence::{session_dir_in_root, test_transcript},
    SessionListing, SessionSummary,
};
use super::*;

const ID: &str = "11111111-2222-4333-8444-555555555555";

/// When the transcripts here were last saved: long before any test runs,
/// so a mark that moved `updated_at` to its own time would show.
const SAVED_AT: u64 = 1_700_000_000;

/// Saves session [`ID`] in `cwd` under `root`, its transcript holding
/// `prompts`, last saved at `saved_at`.
fn save(root: &Path, cwd: &Path, saved_at: u64, prompts: &[&str]) {
    let dir = session_dir_in_root(root, cwd);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{SAVED_AT}_{ID}.jsonl")),
        test_transcript(cwd, ID, saved_at, prompts),
    )
    .unwrap();
}

/// The one session saved in `cwd` under `root`, as a list reports it.
fn listed(root: &Path, cwd: &Path) -> SessionSummary {
    let sessions = Session::list_in_root_for_test(root, cwd, SessionListing::Everything).unwrap();
    let [session] = <[_; 1]>::try_from(sessions).expect("one session is saved");
    session
}

/// Asks for `asked` as session [`ID`]'s mark; `None` clears it.
fn mark(root: &Path, cwd: &Path, asked: Option<NewMark>) -> MarkChange {
    change_mark(root, &workspace_key(cwd), ID, |_, now| {
        asked.map(|mark| mark.at(now))
    })
    .unwrap()
}

// Covers: a mark is saved with the session and listed with it; setting,
// replacing and clearing it leave everything else the list reports as it
// was, `updated_at` above all, which clients compare a done mark with; a
// later save of the transcript moves `updated_at` and keeps the mark; and
// the store stamps the time a mark is set, on its own clock, taking only a
// snooze's end from the caller. A mark written to the transcript, or lost
// to a re-summary, fails here. Owner: session store.
#[test]
fn a_mark_is_listed_and_leaves_the_rest_of_the_session_as_it_was() {
    let (root, cwd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let (root, cwd) = (root.path(), cwd.path());
    save(root, cwd, SAVED_AT, &["first"]);
    let saved = listed(root, cwd);

    let done = mark(root, cwd, Some(NewMark::Done));
    let Some(SessionMark::Done { at }) = done.after else {
        panic!("the session is marked done, not {:?}", done.after);
    };
    let marked = listed(root, cwd);
    let snoozed = mark(
        root,
        cwd,
        Some(NewMark::Snoozed {
            until: SAVED_AT + 60,
        }),
    );
    save(root, cwd, SAVED_AT + 500, &["first", "second"]);
    let saved_again = listed(root, cwd);
    let cleared = mark(root, cwd, None);
    let unmarked = listed(root, cwd);

    let snooze = Some(SessionMark::Snoozed {
        until: SAVED_AT + 60,
    });
    let later = SessionSummary {
        // `test_transcript` stamps the header it rewrites with the save's
        // time too.
        created_at: SAVED_AT + 500,
        updated_at: SAVED_AT + 500,
        message_count: 2,
        last_user_message: Some("second".into()),
        ..saved.clone()
    };
    assert_eq!(
        (
            (saved.updated_at, saved.mark, at > SAVED_AT),
            (done.before, marked),
            snoozed,
            saved_again,
            cleared,
            unmarked,
        ),
        (
            (SAVED_AT, None, true),
            (
                None,
                SessionSummary {
                    mark: done.after,
                    ..saved
                }
            ),
            MarkChange {
                before: done.after,
                after: snooze,
            },
            SessionSummary {
                mark: snooze,
                ..later.clone()
            },
            MarkChange {
                before: snooze,
                after: None,
            },
            later,
        )
    );
}

// Covers: a session nothing is saved under has no mark to change, which
// the daemon answers as `SessionNotFound`. Owner: session store.
#[test]
fn a_missing_session_has_no_mark_to_change() {
    let (root, cwd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let (root, cwd) = (root.path(), cwd.path());
    save(root, cwd, SAVED_AT, &["first"]);

    let missing = change_mark(root, &workspace_key(cwd), "no-such-session", |_, _| None)
        .map_err(|error| error.is::<NoSessionFound>());

    assert_eq!(missing.map(|_| ()), Err(true));
}

// Covers: a mark this Rho cannot read, as a later Rho may save, reads as
// none rather than failing the list for every session beside it, and
// clearing it removes it, though it read as none already.
// Owner: session store.
#[test]
fn an_unreadable_mark_reads_as_none_and_can_be_cleared() {
    let (root, cwd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let (root, cwd) = (root.path(), cwd.path());
    save(root, cwd, SAVED_AT, &["first"]);
    let saved = listed(root, cwd);
    let column = || {
        let index = open_index(root).unwrap();
        let index = index.lock().unwrap();
        index
            .query_row("select mark from sessions where id = ?1", [ID], |row| {
                row.get::<_, Option<String>>(0)
            })
            .unwrap()
    };
    let newer = r#"{"kind":"archived","at":1}"#;
    open_index(root)
        .unwrap()
        .lock()
        .unwrap()
        .execute(
            "update sessions set mark = ?1 where id = ?2",
            params![newer, ID],
        )
        .unwrap();

    let read = (listed(root, cwd), column());
    mark(root, cwd, None);

    assert_eq!((read, column()), ((saved, Some(newer.to_owned())), None));
}
