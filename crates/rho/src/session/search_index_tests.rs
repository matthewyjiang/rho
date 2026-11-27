use std::{cell::RefCell, sync::mpsc};

use pretty_assertions::assert_eq;
use rho_providers::model::Message;
use tempfile::TempDir;

use super::*;
use crate::{app::row_locations, session::Session};

struct MigrationWait {
    blocked: mpsc::Sender<()>,
    resume: mpsc::Receiver<()>,
}

thread_local! {
    static MIGRATION_WAIT: RefCell<Option<MigrationWait>> = const { RefCell::new(None) };
}

// Covers: a waiting opener must not invalidate evidence rebuilt by the first
// migration. Owner: search index SQLite migration, with actual lock contention.
#[test]
fn concurrent_migration_preserves_rebuilt_cache() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    session
        .append_message(&Message::user_text("migrationneedle"))
        .unwrap();
    let cancellation = CancellationToken::new();
    let mut first = open(root.path()).unwrap();
    refresh(
        &mut first,
        root.path(),
        /*reconcile*/ false,
        row_locations,
        &cancellation,
    )
    .unwrap();
    first.pragma_update(None, "user_version", 1).unwrap();
    let (blocked_tx, blocked_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let lock = first
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let root = root.path();
        let second = scope.spawn(move || {
            MIGRATION_WAIT.with(|wait| {
                *wait.borrow_mut() = Some(MigrationWait {
                    blocked: blocked_tx,
                    resume: resume_rx,
                });
            });
            let mut connection = Connection::open(root.join("search.sqlite3")).unwrap();
            connection
                .busy_handler(Some(|_| {
                    MIGRATION_WAIT.with(|wait| {
                        let Some(wait) = wait.borrow_mut().take() else {
                            return false;
                        };
                        // Use the store's existing lock budget only as a failure
                        // bound. Channels, not elapsed time, order the migration.
                        wait.blocked.send(()).is_ok()
                            && wait
                                .resume
                                .recv_timeout(crate::sqlite_support::BUSY_TIMEOUT)
                                .is_ok()
                    })
                }))
                .unwrap();
            migrate(&mut connection).unwrap();
            refresh(
                &mut connection,
                root,
                /*reconcile*/ false,
                row_locations,
                &CancellationToken::new(),
            )
            .unwrap()
        });
        blocked_rx
            .recv_timeout(crate::sqlite_support::BUSY_TIMEOUT)
            .unwrap();
        lock.rollback().unwrap();
        migrate(&mut first).unwrap();
        let rebuilt = refresh(
            &mut first,
            root,
            /*reconcile*/ false,
            row_locations,
            &cancellation,
        )
        .unwrap();
        assert_eq!(rebuilt.files_updated, 1);
        resume_tx.send(()).unwrap();
        let warm = second.join().unwrap();
        assert_eq!(
            (warm.reconciled, warm.files_checked, warm.bytes_read),
            (false, 0, 0)
        );
        assert_eq!(
            first
                .query_row(
                    "select count(*) from evidence_fts where evidence_fts match 'migrationneedle'",
                    [],
                    |row| row.get::<_, usize>(0)
                )
                .unwrap(),
            1
        );
    });
}

// Covers: the cache is rewritten only when both the absolute and relative
// free-space thresholds hold. Owner: search index vacuum policy.
#[test]
fn vacuum_requires_both_free_space_thresholds() {
    const MIB: u64 = 1024 * 1024;
    let pages = |total_mib: u64, free_mib: u64| Pages {
        size: 4096,
        total: total_mib * MIB / 4096,
        free: free_mib * MIB / 4096,
    };
    let cases = [
        ("small cache, mostly free", pages(60, 40), false),
        ("large cache, small fraction free", pages(1000, 100), false),
        ("both thresholds at the boundary", pages(200, 50), true),
        ("observed bloated cache", pages(331, 165), true),
    ];
    let actual: Vec<_> = cases
        .iter()
        .map(|(name, pages, _)| (*name, pages.worth_vacuuming(VACUUM_MIN_FREE_BYTES)))
        .collect();
    let expected: Vec<_> = cases.iter().map(|(name, _, want)| (*name, *want)).collect();
    assert_eq!(actual, expected);
}

// Covers: deleted sessions leave freed pages that only VACUUM returns to the
// OS, without losing surviving evidence. Owner: search index compaction
// against a real SQLite file.
#[test]
fn vacuum_returns_deleted_session_pages_to_the_filesystem() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let cancellation = CancellationToken::new();
    let filler = "evidence ".repeat(64 * 1024);
    let sessions: Vec<_> = (0..4)
        .map(|index| {
            let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
            session
                .append_message(&Message::user_text(format!("vacuumneedle{index} {filler}")))
                .unwrap();
            session
        })
        .collect();
    let mut connection = open(root.path()).unwrap();
    refresh(
        &mut connection,
        root.path(),
        /*reconcile*/ false,
        row_locations,
        &cancellation,
    )
    .unwrap();
    for session in &sessions[1..] {
        fs::remove_file(session.path()).unwrap();
    }
    // Real refreshes vacuum past 50 MiB free; this cache stays below that.
    refresh(
        &mut connection,
        root.path(),
        /*reconcile*/ true,
        row_locations,
        &cancellation,
    )
    .unwrap();
    // Flush WAL frames so the file size reflects every allocated page.
    connection
        .query_row("pragma wal_checkpoint(truncate)", [], |_| Ok(()))
        .unwrap();
    let database = root.path().join("search.sqlite3");
    let bloated = Pages::read(&connection).unwrap();
    assert_eq!(
        fs::metadata(&database).unwrap().len(),
        bloated.total * bloated.size
    );
    assert!(bloated.worth_vacuuming(/*min_free_bytes*/ 0));

    vacuum_if_bloated(&connection, /*min_free_bytes*/ 0).unwrap();

    let vacuumed = Pages::read(&connection).unwrap();
    assert_eq!(
        (
            vacuumed.free,
            vacuumed.total,
            fs::metadata(&database).unwrap().len()
        ),
        (
            0,
            bloated.total - bloated.free,
            (bloated.total - bloated.free) * bloated.size
        )
    );
    assert_eq!(
        connection
            .query_row(
                "select count(*) from evidence_fts where evidence_fts match 'vacuumneedle0'",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
        1
    );
}

// Covers: evidence an older schema indexed from a session whose header is
// format 1-3 must not stay searchable once rho refuses to open that session,
// even though the file's size and mtime still match the cached stamp.
// Owner: search index SQLite migration.
#[test]
fn schema_upgrade_drops_evidence_from_pre_tree_sessions() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    session
        .append_message(&Message::user_text("pretreeneedle"))
        .unwrap();
    let cancellation = CancellationToken::new();
    let matches = |connection: &Connection| {
        connection
            .query_row(
                "select count(*) from evidence_fts where evidence_fts match 'pretreeneedle'",
                [],
                |row| row.get::<_, usize>(0),
            )
            .unwrap()
    };
    let mut connection = open(root.path()).unwrap();
    refresh(
        &mut connection,
        root.path(),
        /*reconcile*/ false,
        row_locations,
        &cancellation,
    )
    .unwrap();
    assert_eq!(matches(&connection), 1);

    // The cache an earlier rho 3 left: schema 4, with evidence from a file
    // whose header (same size, same mtime) declares format 2.
    let modified = fs::metadata(session.path()).unwrap().modified().unwrap();
    let transcript = fs::read_to_string(session.path()).unwrap();
    fs::write(
        session.path(),
        transcript.replacen("\"version\":5", "\"version\":2", 1),
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(session.path())
        .unwrap()
        .set_modified(modified)
        .unwrap();
    connection.pragma_update(None, "user_version", 4).unwrap();
    drop(connection);

    let mut connection = open(root.path()).unwrap();
    refresh(
        &mut connection,
        root.path(),
        /*reconcile*/ false,
        row_locations,
        &cancellation,
    )
    .unwrap();
    assert_eq!(matches(&connection), 0);
}

// Covers: a transcript that does not load is not indexed, and is not read
// again on every refresh, which a search runs before each query, until it
// changes. Owner: search index refresh.
#[test]
fn a_transcript_that_does_not_load_is_read_once() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    session
        .append_message(&Message::user_text("brokenneedle"))
        .unwrap();
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(session.path())
        .unwrap();
    std::io::Write::write_all(&mut file, b"{\"type\":\"node\",\n").unwrap();
    let cancellation = CancellationToken::new();
    let mut connection = open(root.path()).unwrap();
    let report = |refresh: Refresh| {
        (
            refresh.files_checked,
            refresh.files_updated,
            refresh.skipped_files,
            refresh.omitted_records,
        )
    };

    let first = refresh(
        &mut connection,
        root.path(),
        /*reconcile*/ false,
        row_locations,
        &cancellation,
    )
    .unwrap();
    let second = refresh(
        &mut connection,
        root.path(),
        /*reconcile*/ false,
        row_locations,
        &cancellation,
    )
    .unwrap();

    let matches: usize = connection
        .query_row(
            "select count(*) from evidence_fts where evidence_fts match 'brokenneedle'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(
        (report(first), report(second), matches),
        ((1, 1, 0, 1), (0, 0, 0, 1), 0)
    );
}

// Covers: a transcript whose read failed, as on a full or unmounted disk,
// is read again once reads work, not cached as one that does not load, and
// the index reports itself incomplete meanwhile, which a daemon search
// answers as `indexing`. Owner: search index refresh.
#[cfg(unix)]
#[test]
fn a_transcript_whose_read_failed_is_read_again() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    session
        .append_message(&Message::user_text("retryneedle"))
        .unwrap();
    let readable = |mode| fs::set_permissions(session.path(), fs::Permissions::from_mode(mode));
    readable(0o000).unwrap();
    // Root or CAP_DAC_OVERRIDE can still open 0o000 files; skip if this process can.
    if fs::File::open(session.path()).is_ok() {
        readable(0o600).unwrap();
        return;
    }
    let cancellation = CancellationToken::new();
    let mut connection = open(root.path()).unwrap();
    let state = |connection: &mut Connection| {
        refresh(
            connection,
            root.path(),
            /*reconcile*/ false,
            row_locations,
            &cancellation,
        )
        .unwrap();
        let matches: usize = connection
            .query_row(
                "select count(*) from evidence_fts where evidence_fts match 'retryneedle'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        (complete(connection).unwrap(), matches)
    };

    let failed = state(&mut connection);
    readable(0o600).unwrap();
    let recovered = state(&mut connection);

    assert_eq!((failed, recovered), ((false, 0), (true, 1)));
}

// Covers: which indexing failures a later refresh retries: failed reads and
// a transcript that changed while read, wrapped or not, but not a
// transcript that is malformed, text that is not UTF-8 included, which
// would otherwise keep a search `indexing` for good. Owner: search index
// refresh.
#[test]
fn only_failed_reads_and_changes_are_retried() {
    use std::io::{Error, ErrorKind};

    let cases: [(&str, anyhow::Error, bool); 6] = [
        (
            "failed read",
            Error::from(ErrorKind::PermissionDenied).into(),
            true,
        ),
        (
            "failed read with context",
            anyhow::Error::from(Error::from(ErrorKind::Other)).context("loading"),
            true,
        ),
        ("changed while read", ChangedWhileRead.into(), true),
        (
            "not UTF-8",
            Error::from(ErrorKind::InvalidData).into(),
            false,
        ),
        (
            "malformed record",
            serde_json::from_str::<serde_json::Value>("{")
                .unwrap_err()
                .into(),
            false,
        ),
        (
            "invalid header",
            anyhow::anyhow!("missing session header"),
            false,
        ),
    ];

    let actual: Vec<_> = cases
        .iter()
        .map(|(name, error, _)| (*name, transient(error)))
        .collect();
    let expected: Vec<_> = cases
        .iter()
        .map(|(name, _, retried)| (*name, *retried))
        .collect();
    assert_eq!(actual, expected);
}
