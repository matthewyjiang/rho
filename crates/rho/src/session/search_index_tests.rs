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
