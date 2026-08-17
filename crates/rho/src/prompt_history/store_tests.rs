use pretty_assertions::assert_eq;
use rusqlite::Connection;
use tempfile::TempDir;

use super::*;

fn store() -> (TempDir, PromptHistoryStore) {
    let directory = tempfile::tempdir().unwrap();
    let store = PromptHistoryStore::new(directory.path().join("prompt-history.sqlite3")).unwrap();
    (directory, store)
}

// Covers: appends survive a reopen and load oldest-first.
// Owner: prompt history store
#[test]
fn append_round_trips_oldest_first() {
    let (_directory, store) = store();
    store.append("first", 1, 10).unwrap();
    store.append("second", 2, 10).unwrap();

    assert_eq!(
        store.load_tail(10).unwrap(),
        vec!["first".to_string(), "second".to_string()]
    );

    let reopened = PromptHistoryStore::new(store.path()).unwrap();
    assert_eq!(
        reopened.load_tail(10).unwrap(),
        vec!["first".to_string(), "second".to_string()]
    );
}

// Covers: trim happens in the same write as the insert and keeps the newest N.
// Owner: prompt history store
#[test]
fn append_trims_to_newest_max_entries() {
    let (_directory, store) = store();
    for (index, text) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
        store
            .append(text, i64::try_from(index).unwrap(), 3)
            .unwrap();
    }

    assert_eq!(
        store.load_tail(10).unwrap(),
        vec!["c".to_string(), "d".to_string(), "e".to_string()]
    );
}

// Covers: consecutive duplicates are not re-inserted; non-consecutive ones are.
// Owner: prompt history store
#[test]
fn consecutive_duplicate_is_skipped() {
    let (_directory, store) = store();
    store.append("hello", 1, 10).unwrap();
    store.append("hello", 2, 10).unwrap();
    store.append("world", 3, 10).unwrap();
    store.append("hello", 4, 10).unwrap();

    assert_eq!(
        store.load_tail(10).unwrap(),
        vec![
            "hello".to_string(),
            "world".to_string(),
            "hello".to_string()
        ]
    );
}

// Covers: load_tail smaller than stored returns the newest N, oldest first.
// Owner: prompt history store
#[test]
fn load_tail_returns_newest_n() {
    let (_directory, store) = store();
    store.append("one", 1, 10).unwrap();
    store.append("two", 2, 10).unwrap();
    store.append("three", 3, 10).unwrap();

    assert_eq!(
        store.load_tail(2).unwrap(),
        vec!["two".to_string(), "three".to_string()]
    );
}

// Covers: clear empties the table and later appends still work.
// Owner: prompt history store
#[test]
fn clear_empties_then_accepts_appends() {
    let (_directory, store) = store();
    store.append("keep", 1, 10).unwrap();
    store.clear().unwrap();
    assert!(store.load_tail(10).unwrap().is_empty());

    store.append("after", 2, 10).unwrap();
    assert_eq!(store.load_tail(10).unwrap(), vec!["after".to_string()]);
}

// Covers: two handles on one path can append without error.
// Owner: prompt history store
#[test]
fn concurrent_handles_append_without_error() {
    let (_directory, first) = store();
    let second = PromptHistoryStore::new(first.path()).unwrap();
    first.append("from-first", 1, 10).unwrap();
    second.append("from-second", 2, 10).unwrap();

    assert_eq!(
        first.load_tail(10).unwrap(),
        vec!["from-first".to_string(), "from-second".to_string()]
    );
}

// Covers: a newer user_version is refused instead of silently rewriting.
// Owner: prompt history store
#[test]
fn newer_schema_is_unsupported() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prompt-history.sqlite3");
    let store = PromptHistoryStore::new(&path).unwrap();
    drop(store);

    let connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "user_version", super::migrations::SCHEMA_VERSION + 1)
        .unwrap();
    drop(connection);

    let error = PromptHistoryStore::new(&path).unwrap_err();
    assert!(matches!(
        error,
        PromptHistoryError::UnsupportedSchema {
            found,
            supported
        } if found == super::migrations::SCHEMA_VERSION + 1
            && supported == super::migrations::SCHEMA_VERSION
    ));
}

#[cfg(unix)]
// Covers: the history file is created owner-only.
// Owner: prompt history store
#[test]
fn database_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let (_directory, store) = store();
    let mode = std::fs::metadata(store.path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}
