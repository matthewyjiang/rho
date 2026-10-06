use pretty_assertions::assert_eq;
use rho_tools::WorkspaceMutationObserver;

use super::*;

// Covers: broad patches, including absent/empty paths, must release both maps
// at the session budget and still allow every native mutation to complete.
// Owner: turn-scoped capture (append-only quota tests miss peak memory).
#[tokio::test]
async fn many_file_capture_drops_turn_at_remaining_session_budget() -> anyhow::Result<()> {
    #[derive(Clone, Copy)]
    enum ExhaustAt {
        Original,
        ExpectedAfter,
    }
    for original in [Some(b"1234".as_slice()), Some(b"".as_slice()), None] {
        for exhaust_at in [ExhaustAt::Original, ExhaustAt::ExpectedAfter] {
            let temp = tempfile::tempdir()?;
            let workspace = temp.path().join("workspace");
            fs::create_dir(&workspace)?;
            let session = Session::create_in_root(&temp.path().join("sessions"), &workspace)?;
            let mut store = session.workspace_checkpoint_store()?.unwrap();
            let earlier = store.finalize(
                store.open(NodeId::new())?,
                Revision::from_u64(1),
                CheckpointOutcome::Completed,
            )?;
            let journal = fs::read(&store.journal_path)?;
            let journal_bytes = journal.len() as u64;
            // Size the quota from reservations: three complete file pairs fit,
            // then the fourth original or fourth expected-after trips it.
            let paths = (0..5)
                .map(|index| workspace.join(format!("file-{index}.txt")))
                .collect::<Vec<_>>();
            let content_bytes =
                budget::encoded_content_bytes(original.map_or(0, |bytes| bytes.len() as u64));
            let entry_bytes = budget::entry_bytes(&paths[0]);
            let original_bytes = entry_bytes + content_bytes;
            let pair_bytes = original_bytes + entry_bytes;
            let remaining = 3 * pair_bytes
                + match exhaust_at {
                    ExhaustAt::Original => 0,
                    ExhaustAt::ExpectedAfter => original_bytes,
                };
            store.limits = CheckpointLimits {
                max_file_bytes: 4,
                max_session_bytes: journal_bytes + remaining,
            };
            let tracker = WorkspaceCheckpointTracker::new(true);
            *tracker.active.lock().unwrap() = Some(ActiveCheckpoint {
                open: store.open(NodeId::new())?,
                store: store.clone(),
            });
            for (index, path) in paths.iter().enumerate() {
                if let Some(bytes) = original {
                    fs::write(path, bytes)?;
                }
                tracker
                    .before_mutation(&[path.as_path()])
                    .await
                    .map_err(anyhow::Error::msg)?;
                fs::write(path, b"done")?;
                tracker
                    .after_mutation(&[path.as_path()])
                    .await
                    .map_err(anyhow::Error::msg)?;
                let active = tracker.active.lock().unwrap();
                let open = &active.as_ref().unwrap().open;
                if index < 3 {
                    assert_eq!(open.captured_bytes, (index as u64 + 1) * pair_bytes);
                    assert!(open.quota_exceeded.is_none());
                } else {
                    assert_eq!(
                        (
                            open.captured_bytes,
                            open.originals.len(),
                            open.expected_after.len(),
                            open.limitations.len()
                        ),
                        (0, 0, 0, 0),
                    );
                    assert!(open.quota_exceeded.is_some());
                }
            }
            let error = tracker
                .finalize_turn(
                    NodeId::new(),
                    Revision::from_u64(2),
                    CheckpointOutcome::Completed,
                )
                .unwrap_err()
                .downcast::<CheckpointAppendError>()?;
            let CheckpointAppendError::QuotaExceeded {
                asked,
                limit,
                turn_bytes,
            } = error
            else {
                panic!("expected quota error, got {error:?}");
            };
            let attempted = 3 * pair_bytes
                + match exhaust_at {
                    ExhaustAt::Original => entry_bytes,
                    ExhaustAt::ExpectedAfter => pair_bytes,
                };
            assert_eq!(
                (asked, limit, turn_bytes),
                (
                    journal_bytes + attempted,
                    journal_bytes + remaining,
                    attempted
                ),
            );
            for path in paths {
                assert_eq!(fs::read(path)?, b"done");
            }
            assert_eq!(fs::read(&store.journal_path)?, journal);
            assert_eq!(store.list()?.len(), 1);
            assert_eq!(store.get(&earlier.node_id)?, Some(earlier));
        }
    }
    Ok(())
}
