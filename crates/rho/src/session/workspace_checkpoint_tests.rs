use std::{collections::BTreeMap, fs, io::Write as _, path::PathBuf};

use pretty_assertions::assert_eq;
use rho_sdk::Revision;
use tempfile::TempDir;

use super::*;

fn checkpoint_store(session: &Session) -> anyhow::Result<WorkspaceCheckpointStore> {
    session
        .workspace_checkpoint_store()?
        .ok_or_else(|| anyhow::anyhow!("checkpoint store is unavailable"))
}

fn test_session() -> anyhow::Result<(TempDir, Session, PathBuf)> {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace)?;
    let session = Session::create_in_root(&temp.path().join("sessions"), &workspace)?;
    Ok((temp, session, workspace))
}

// Covers: a torn final append must not hide earlier checkpoints or block the next append.
// Owner: session checkpoint persistence
#[test]
fn checkpoint_journal_persists_binary_state_and_recovers_a_torn_tail() -> anyhow::Result<()> {
    let (_temp, session, workspace) = test_session()?;
    let path = workspace.join("binary.dat");
    fs::write(&path, [0, 0xff, 0x80, b'\n'])?;
    let store = checkpoint_store(&session)?;

    let first_node = NodeId::new();
    let mut open = store.open(first_node.clone());
    assert_eq!(open.capture_path(&path), CaptureDisposition::Captured);
    assert_eq!(
        open.capture_path(&path),
        CaptureDisposition::AlreadyCaptured
    );
    open.record_untracked_effect(UntrackedEffect {
        kind: UntrackedEffectKind::ShellCommand,
        source: "shell".to_string(),
    });
    fs::write(&path, [0xfe, 0, 0x81])?;
    let first = store.finalize(open, Revision::from_u64(4), CheckpointOutcome::Completed)?;

    let reopened = checkpoint_store(&session)?;
    assert_eq!(reopened.get(&first_node)?, Some(first.clone()));
    assert_eq!(
        reopened.observe_current(&first),
        first
            .files
            .iter()
            .map(|file| (file.path.clone(), file.expected_after.clone()))
            .collect()
    );
    let OriginalFileState::Regular(original) = &first.files[0].original else {
        panic!("binary file was not captured as a regular file");
    };
    assert_eq!(original.bytes, vec![0, 0xff, 0x80, b'\n']);
    assert_eq!(original.digest.0.len(), 64);

    OpenOptions::new()
        .append(true)
        .open(&store.journal_path)?
        .write_all(br#"{"version":1,"checkpoint":{"torn""#)?;
    assert_eq!(reopened.list()?, vec![first.clone()]);
    OpenOptions::new()
        .append(true)
        .open(&store.journal_path)?
        .write_all(b"}\n")?;
    assert_eq!(reopened.list()?, vec![first.clone()]);

    let second_path = workspace.join("created.txt");
    let second_node = NodeId::new();
    let mut second_open = reopened.open(second_node.clone());
    second_open.capture_path(&second_path);
    fs::write(&second_path, b"created")?;
    let second = reopened.finalize(
        second_open,
        Revision::from_u64(5),
        CheckpointOutcome::Cancelled,
    )?;
    assert_eq!(reopened.list()?, vec![first, second]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&store.checkpoint_dir)?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&store.journal_path)?.permissions().mode() & 0o777,
            0o600
        );
    }
    Ok(())
}

// Covers: a readable future journal version must never be treated as a truncatable torn tail.
// Owner: session checkpoint persistence
#[test]
fn checkpoint_journal_preserves_unknown_versions() -> anyhow::Result<()> {
    let (_temp, session, workspace) = test_session()?;
    let path = workspace.join("tracked.txt");
    fs::write(&path, b"original")?;
    let store = checkpoint_store(&session)?;
    let mut open = store.open(NodeId::new());
    open.capture_path(&path);
    fs::write(&path, b"agent")?;
    let first = store.finalize(open, Revision::from_u64(1), CheckpointOutcome::Completed)?;
    let mut future = serde_json::to_vec(&StoredCheckpointRecord {
        version: CHECKPOINT_FORMAT_VERSION + 1,
        checkpoint: first,
    })?;
    future.push(b'\n');
    OpenOptions::new()
        .append(true)
        .open(&store.journal_path)?
        .write_all(&future)?;
    let before_append = fs::read(&store.journal_path)?;

    assert!(store.list().is_err());
    let mut next = store.open(NodeId::new());
    next.capture_path(&workspace.join("next.txt"));
    assert!(store
        .finalize(next, Revision::from_u64(2), CheckpointOutcome::Completed,)
        .is_err());
    assert_eq!(fs::read(&store.journal_path)?, before_append);
    Ok(())
}

fn metadata() -> BasicFileMetadata {
    BasicFileMetadata {
        readonly: false,
        unix_mode: None,
    }
}

fn captured(bytes: &[u8]) -> OriginalFileState {
    OriginalFileState::Regular(CapturedRegularFile {
        bytes: bytes.to_vec(),
        metadata: metadata(),
        digest: FileDigest::for_bytes(bytes),
    })
}

fn observed(bytes: &[u8]) -> ObservedFileState {
    ObservedFileState::Regular {
        digest: FileDigest::for_bytes(bytes),
        size: bytes.len() as u64,
        metadata: metadata(),
    }
}

// Covers: preview must classify each safe restore action and refuse external changes.
// Owner: pure workspace restore policy
#[test]
fn restore_plan_classifies_actions_conflicts_unsupported_and_binary_state() -> anyhow::Result<()> {
    struct Case {
        name: &'static str,
        original: OriginalFileState,
        expected: ObservedFileState,
        current: ObservedFileState,
        classification: RestoreClassification,
    }

    let cases = [
        Case {
            name: "create",
            original: captured(&[0, 0xff]),
            expected: ObservedFileState::Absent,
            current: ObservedFileState::Absent,
            classification: RestoreClassification::Create,
        },
        Case {
            name: "modify binary",
            original: captured(&[0, 0xff, 0x80]),
            expected: observed(&[0xfe, 0, 0x81]),
            current: observed(&[0xfe, 0, 0x81]),
            classification: RestoreClassification::Modify,
        },
        Case {
            name: "delete",
            original: OriginalFileState::Absent,
            expected: observed(b"new"),
            current: observed(b"new"),
            classification: RestoreClassification::Delete,
        },
        Case {
            name: "conflict",
            original: captured(b"before"),
            expected: observed(b"agent"),
            current: observed(b"external"),
            classification: RestoreClassification::Conflict,
        },
        Case {
            name: "unsupported",
            original: OriginalFileState::Unsupported {
                reason: UnsupportedPath::Symlink,
            },
            expected: ObservedFileState::Unsupported {
                reason: UnsupportedPath::Symlink,
            },
            current: ObservedFileState::Unsupported {
                reason: UnsupportedPath::Symlink,
            },
            classification: RestoreClassification::Unsupported,
        },
        Case {
            name: "skipped",
            original: captured(b"unchanged"),
            expected: observed(b"unchanged"),
            current: observed(b"unchanged"),
            classification: RestoreClassification::Skipped,
        },
    ];

    let files = cases
        .iter()
        .map(|case| FileCheckpoint {
            path: PathBuf::from(case.name),
            original: case.original.clone(),
            expected_after: case.expected.clone(),
        })
        .collect::<Vec<_>>();
    let current = cases
        .iter()
        .map(|case| (PathBuf::from(case.name), case.current.clone()))
        .collect::<BTreeMap<_, _>>();
    let limitation = UntrackedEffect {
        kind: UntrackedEffectKind::UntrackedMutatingTool,
        source: "third-party tool".to_string(),
    };
    let checkpoint = WorkspaceCheckpoint {
        session_id: rho_sdk::SessionId::from_string("session")?,
        node_id: NodeId::from_string("node")?,
        revision: Revision::from_u64(9),
        started_at: 1,
        finalized_at: 2,
        outcome: CheckpointOutcome::Failed,
        files,
        limitations: vec![limitation.clone()],
    };

    let plan = plan_restore(&checkpoint, &current)?;
    assert_eq!(
        plan.entries,
        cases
            .iter()
            .map(|case| RestorePlanEntry {
                path: PathBuf::from(case.name),
                classification: case.classification,
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(plan.limitations, vec![limitation]);
    Ok(())
}

// Covers: restore applies create/modify/delete actions and leaves a concurrent edit unchanged.
// Owner: session checkpoint restore executor
#[test]
fn restore_applies_safe_actions_and_audits_conflicts() -> anyhow::Result<()> {
    let (_temp, session, workspace) = test_session()?;
    let created_before_turn = workspace.join("created-before.txt");
    let modified = workspace.join("modified.bin");
    let deleted_before_turn = workspace.join("deleted-before.txt");
    let conflicted = workspace.join("conflicted.txt");
    fs::write(&created_before_turn, b"original")?;
    fs::write(&modified, [0, 0xff, 0x80])?;
    fs::write(&conflicted, b"original")?;

    let store = checkpoint_store(&session)?;
    let mut open = store.open(NodeId::new());
    for path in [
        &created_before_turn,
        &modified,
        &deleted_before_turn,
        &conflicted,
    ] {
        assert_eq!(open.capture_path(path), CaptureDisposition::Captured);
    }
    fs::remove_file(&created_before_turn)?;
    fs::write(&modified, [0xfe, 0, 0x81])?;
    fs::write(&deleted_before_turn, b"agent-created")?;
    fs::write(&conflicted, b"agent-change")?;
    let checkpoint = store.finalize(open, Revision::from_u64(1), CheckpointOutcome::Completed)?;

    fs::write(&conflicted, b"external-change")?;
    let current = store.observe_current(&checkpoint);
    let audit = store.restore(
        &checkpoint,
        &current,
        |path| store.observe_path(path),
        |file, classification| store.apply_restore(file, classification),
    );

    assert_eq!(fs::read(&created_before_turn)?, b"original");
    assert_eq!(fs::read(&modified)?, [0, 0xff, 0x80]);
    assert!(!deleted_before_turn.exists());
    assert_eq!(fs::read(&conflicted)?, b"external-change");
    assert_eq!(
        audit
            .entries
            .iter()
            .map(|entry| (entry.classification, entry.changed, entry.error.is_some()))
            .collect::<Vec<_>>(),
        vec![
            (RestoreClassification::Conflict, false, false),
            (RestoreClassification::Create, true, false),
            (RestoreClassification::Delete, true, false),
            (RestoreClassification::Modify, true, false),
        ]
    );
    Ok(())
}

// Covers: restoring a deleted file must recreate its missing parent directory.
// Owner: session checkpoint restore executor
#[test]
fn restore_recreates_missing_parent_directory() -> anyhow::Result<()> {
    let (_temp, session, workspace) = test_session()?;
    let path = workspace.join("removed-parent").join("tracked.txt");
    fs::create_dir(path.parent().expect("test path must have a parent"))?;
    fs::write(&path, b"original")?;
    let store = checkpoint_store(&session)?;
    let mut open = store.open(NodeId::new());
    open.capture_path(&path);
    fs::remove_dir_all(path.parent().expect("test path must have a parent"))?;
    let checkpoint = store.finalize(open, Revision::from_u64(1), CheckpointOutcome::Completed)?;
    let current = store.observe_current(&checkpoint);

    let audit = store.restore(
        &checkpoint,
        &current,
        |path| store.observe_path(path),
        |file, classification| store.apply_restore(file, classification),
    );

    assert_eq!(fs::read(&path)?, b"original");
    assert_eq!(
        audit.entries,
        vec![RestoreAuditEntry {
            path,
            classification: RestoreClassification::Create,
            changed: true,
            error: None,
        }]
    );
    Ok(())
}

// Covers: a path changed after preview must become a conflict before the restore write.
// Owner: session checkpoint restore executor
#[test]
fn restore_rechecks_state_after_preview() -> anyhow::Result<()> {
    let (_temp, session, workspace) = test_session()?;
    let path = workspace.join("raced.txt");
    fs::write(&path, b"original")?;
    let store = checkpoint_store(&session)?;
    let mut open = store.open(NodeId::new());
    open.capture_path(&path);
    fs::write(&path, b"agent-change")?;
    let checkpoint = store.finalize(open, Revision::from_u64(1), CheckpointOutcome::Completed)?;
    let previewed = store.observe_current(&checkpoint);

    fs::write(&path, b"changed-after-preview")?;
    let audit = store.restore(
        &checkpoint,
        &previewed,
        |path| store.observe_path(path),
        |file, classification| store.apply_restore(file, classification),
    );

    assert_eq!(fs::read(&path)?, b"changed-after-preview");
    assert_eq!(
        audit.entries,
        vec![RestoreAuditEntry {
            path,
            classification: RestoreClassification::Conflict,
            changed: false,
            error: None,
        }]
    );
    Ok(())
}

// Covers: a symlink that replaces a restore target must not redirect writes outside the target path.
// Owner: OS checkpoint restore boundary
#[cfg(unix)]
#[test]
fn restore_does_not_follow_a_replacement_symlink() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;

    let (_temp, session, workspace) = test_session()?;
    let target = workspace.join("target.txt");
    let outside = workspace.parent().unwrap().join("outside.txt");
    fs::write(&target, b"original")?;
    fs::write(&outside, b"outside")?;
    let store = checkpoint_store(&session)?;
    let mut open = store.open(NodeId::new());
    open.capture_path(&target);
    fs::write(&target, b"agent-change")?;
    let checkpoint = store.finalize(open, Revision::from_u64(1), CheckpointOutcome::Completed)?;

    fs::remove_file(&target)?;
    symlink(&outside, &target)?;
    let current = store.observe_current(&checkpoint);
    let audit = store.restore(
        &checkpoint,
        &current,
        |path| store.observe_path(path),
        |file, classification| store.apply_restore(file, classification),
    );

    assert_eq!(fs::read(&outside)?, b"outside");
    assert_eq!(fs::read_link(&target)?, outside);
    assert_eq!(
        audit.entries,
        vec![RestoreAuditEntry {
            path: target,
            classification: RestoreClassification::Conflict,
            changed: false,
            error: None,
        }]
    );
    Ok(())
}

// Covers: the tool observer captures one turn and finalizes it against its session node.
// Owner: interactive workspace checkpoint lifecycle
#[tokio::test]
async fn tracker_captures_native_mutations_for_the_active_turn() -> anyhow::Result<()> {
    let (_temp, session, workspace) = test_session()?;
    let path = workspace.join("tracked.txt");
    fs::write(&path, b"before")?;
    let tracker = WorkspaceCheckpointTracker::new(true);
    tracker.begin_turn(Some(&session))?;
    rho_tools::WorkspaceMutationObserver::before_mutation(&tracker, &[path.as_path()])
        .await
        .map_err(anyhow::Error::msg)?;
    fs::write(&path, b"after")?;
    rho_tools::WorkspaceMutationObserver::after_mutation(&tracker, &[path.as_path()])
        .await
        .map_err(anyhow::Error::msg)?;
    rho_tools::WorkspaceMutationObserver::mark_untracked_effect(
        &tracker,
        rho_tools::UntrackedWorkspaceEffect::ShellCommand,
        "bash",
    );
    let node_id = NodeId::new();
    tracker.finalize_turn(
        node_id.clone(),
        Revision::from_u64(3),
        CheckpointOutcome::Cancelled,
    )?;

    let checkpoint = checkpoint_store(&session)?
        .get(&node_id)?
        .expect("checkpoint should be durable");
    assert_eq!(checkpoint.outcome, CheckpointOutcome::Cancelled);
    assert_eq!(checkpoint.files.len(), 1);
    assert_eq!(
        checkpoint.limitations,
        vec![UntrackedEffect {
            kind: UntrackedEffectKind::ShellCommand,
            source: "bash".into(),
        }]
    );
    Ok(())
}

// Covers: enabling checkpoints for a legacy flat session must not block the provider turn.
// Owner: interactive checkpoint lifecycle
#[test]
fn legacy_flat_session_skips_checkpoint_tracking() -> anyhow::Result<()> {
    let (temp, session, _workspace) = test_session()?;
    let mut legacy = session;
    legacy.path = temp.path().join("1_legacy.jsonl");
    fs::write(&legacy.path, b"")?;
    let tracker = WorkspaceCheckpointTracker::new(true);

    tracker.begin_turn(Some(&legacy))?;

    assert_eq!(
        tracker.finalize_turn(
            NodeId::new(),
            Revision::from_u64(1),
            CheckpointOutcome::Completed,
        )?,
        None
    );
    Ok(())
}

// Covers: an external edit after the last native mutation must not become expected agent state.
// Owner: interactive checkpoint lifecycle
#[tokio::test]
async fn tracker_records_expected_state_after_each_native_mutation() -> anyhow::Result<()> {
    let (_temp, session, workspace) = test_session()?;
    let path = workspace.join("tracked.txt");
    fs::write(&path, b"original")?;
    let tracker = WorkspaceCheckpointTracker::new(true);
    tracker.begin_turn(Some(&session))?;
    rho_tools::WorkspaceMutationObserver::before_mutation(&tracker, &[path.as_path()])
        .await
        .map_err(anyhow::Error::msg)?;
    fs::write(&path, b"agent")?;
    rho_tools::WorkspaceMutationObserver::after_mutation(&tracker, &[path.as_path()])
        .await
        .map_err(anyhow::Error::msg)?;
    let expected_after = observe_path(&path, DEFAULT_MAX_CHECKPOINT_FILE_BYTES);
    fs::write(&path, b"external")?;

    let checkpoint = tracker
        .finalize_turn(
            NodeId::new(),
            Revision::from_u64(1),
            CheckpointOutcome::Completed,
        )?
        .expect("checkpoint should be finalized");

    assert_eq!(checkpoint.files[0].expected_after, expected_after);
    assert_eq!(
        plan_restore(
            &checkpoint,
            &BTreeMap::from([(
                path.clone(),
                observe_path(&path, DEFAULT_MAX_CHECKPOINT_FILE_BYTES),
            )]),
        )?
        .entries[0]
            .classification,
        RestoreClassification::Conflict
    );
    Ok(())
}

// Covers: oversized source files and a full session store must not bypass storage bounds.
// Owner: session checkpoint storage policy
#[test]
fn capture_limit_marks_files_unsupported_and_quota_rejects_append() -> anyhow::Result<()> {
    let (_temp, session, workspace) = test_session()?;
    let path = workspace.join("large.bin");
    fs::write(&path, b"1234")?;
    let limits = CheckpointLimits {
        max_file_bytes: 3,
        max_session_bytes: 1,
    };
    let store = session
        .workspace_checkpoint_store_with_limits(limits)?
        .ok_or_else(|| anyhow::anyhow!("checkpoint store is unavailable"))?;
    let mut open = store.open(NodeId::new());
    open.capture_path(&path);
    fs::write(&path, b"12")?;

    let error = store.finalize(open, Revision::from_u64(1), CheckpointOutcome::Completed);
    assert!(error.is_err());
    assert_eq!(store.list()?, Vec::<WorkspaceCheckpoint>::new());

    let original = capture_original(&path, 1);
    assert_eq!(
        original,
        OriginalFileState::Unsupported {
            reason: UnsupportedPath::TooLarge { size: 2, limit: 1 }
        }
    );
    Ok(())
}
