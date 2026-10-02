use std::{cell::RefCell, sync::mpsc};

use pretty_assertions::assert_eq;
use rho_providers::model::Message;
use tempfile::TempDir;

use super::*;
use crate::session::Session;

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
    refresh(&mut first, root.path(), false, &cancellation).unwrap();
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
            refresh(&mut connection, root, false, &CancellationToken::new()).unwrap()
        });
        blocked_rx
            .recv_timeout(crate::sqlite_support::BUSY_TIMEOUT)
            .unwrap();
        lock.rollback().unwrap();
        migrate(&mut first).unwrap();
        let rebuilt = refresh(&mut first, root, false, &cancellation).unwrap();
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
    refresh(&mut connection, root.path(), false, &cancellation).unwrap();
    for session in &sessions[1..] {
        fs::remove_file(session.path()).unwrap();
    }
    // Real refreshes vacuum past 50 MiB free; this cache stays below that.
    refresh(&mut connection, root.path(), true, &cancellation).unwrap();
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
