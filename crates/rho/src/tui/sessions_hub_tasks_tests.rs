use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::{BatchDeleted, CleanupFailure};

fn failure(id: &str, error: &str, in_use: bool) -> CleanupFailure {
    CleanupFailure {
        id: id.into(),
        cwd: PathBuf::from("/work"),
        error: error.into(),
        in_use,
    }
}

// Covers: only sessions the store refused as open elsewhere are asked of
// the daemon; one it deletes counts as deleted, and one it refuses keeps
// its failure in the daemon's words.
#[test]
fn a_batch_asks_the_daemon_only_for_sessions_held_open() {
    let batch = BatchDeleted {
        deleted: 3,
        failures: vec![
            failure("held-idle", "open in another process", true),
            failure("unreadable", "permission denied", false),
            failure("held-busy", "open in another process", true),
        ],
        ..BatchDeleted::default()
    };

    let batch = batch.retry_held(Some(Box::new(|sessions| {
        assert_eq!(sessions, ["held-idle", "held-busy"]);
        vec![Ok(()), Err("a turn is under way".into())]
    })));

    assert_eq!(batch.deleted, 4);
    assert_eq!(
        batch.failures,
        [
            failure("unreadable", "permission denied", false),
            failure("held-busy", "a turn is under way", true),
        ]
    );
}
