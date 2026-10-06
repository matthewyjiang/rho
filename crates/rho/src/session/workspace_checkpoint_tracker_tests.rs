use pretty_assertions::assert_eq;
use rho_tools::WorkspaceMutationObserver;

use super::*;

// Covers: a broad patch must release its pre-images at the remaining session
// budget, stop collecting, and still allow every native mutation to complete.
// Owner: turn-scoped workspace capture (append-only quota tests miss peak memory).
#[tokio::test]
async fn many_file_capture_drops_turn_at_remaining_session_budget() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace)?;
    let session = Session::create_in_root(&temp.path().join("sessions"), &workspace)?;
    let mut store = session
        .workspace_checkpoint_store()?
        .expect("folder session has checkpoint storage");
    let earlier = store.finalize(
        store.open(NodeId::new())?,
        Revision::from_u64(1),
        CheckpointOutcome::Completed,
    )?;
    let journal = fs::read(&store.journal_path)?;
    let journal_bytes = journal.len() as u64;
    // Three 4-byte pre-images fit; the fourth trips the remaining 12 bytes.
    store.limits = CheckpointLimits {
        max_file_bytes: 4,
        max_session_bytes: journal_bytes + 12,
    };
    let tracker = WorkspaceCheckpointTracker::new(true);
    *tracker.active.lock().unwrap() = Some(ActiveCheckpoint {
        open: store.open(NodeId::new())?,
        store: store.clone(),
    });
    let paths = (0..20)
        .map(|index| workspace.join(format!("file-{index}.txt")))
        .collect::<Vec<_>>();
    for path in &paths {
        fs::write(path, b"1234")?;
    }
    for (index, path) in paths.iter().enumerate() {
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
            assert_eq!(open.captured_bytes, (index as u64 + 1) * 4);
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
    assert_eq!(
        (asked, limit, turn_bytes),
        (journal_bytes + 16, journal_bytes + 12, 16)
    );
    for path in paths {
        assert_eq!(fs::read(path)?, b"done");
    }
    assert_eq!(fs::read(&store.journal_path)?, journal);
    assert_eq!(
        store.list()?,
        vec![WorkspaceCheckpointSummary {
            session_id: earlier.session_id,
            node_id: earlier.node_id,
            before_node_id: earlier.before_node_id,
            revision: earlier.revision,
            started_at: earlier.started_at,
            finalized_at: earlier.finalized_at,
            outcome: earlier.outcome,
            file_count: 0,
            limitations: earlier.limitations,
        }]
    );
    Ok(())
}
