use pretty_assertions::assert_eq;
use rho_providers::model::{Message, ModelIdentity};
use rho_sdk::{CompactionState, Revision, SessionId, SessionSnapshot};

use super::{segments, HistorySegment};
use crate::session::Session;

// Covers: replay sees turns on both sides of a compaction, and marks where the
// post-compaction turns start so kept messages are not replayed twice.
// Owner: session replay reader.
#[test]
fn segments_split_the_active_path_at_each_compaction() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let id = SessionId::from_string(session.id().to_owned()).unwrap();
    let compacted =
        CompactionState::from_accounting(1, 0, 4, 0, Some(8), Some(4), Some(Revision::from_u64(3)));
    let before = vec![Message::user_text("one"), Message::assistant_text("two")];
    let summary = vec![
        Message::user_text("summary"),
        Message::assistant_text("two"),
    ];
    let after = [summary.clone(), vec![Message::user_text("three")]].concat();
    for (revision, history, compaction) in [
        (1, &before[..1], CompactionState::default()),
        (2, &before[..], CompactionState::default()),
        (3, &summary[..], compacted.clone()),
        (4, &after[..], compacted.clone()),
    ] {
        let snapshot = SessionSnapshot::new(
            id.clone(),
            Revision::from_u64(revision),
            history.to_vec(),
            ModelIdentity::new("provider", "api", "model"),
            compaction,
        );
        session.save_snapshot(&snapshot, history).unwrap();
    }

    let (_, found) = segments(session.path()).unwrap();

    assert_eq!(
        found,
        vec![
            HistorySegment {
                messages: before,
                new_from: 0,
            },
            HistorySegment {
                messages: after,
                new_from: 2,
            },
        ]
    );
}
