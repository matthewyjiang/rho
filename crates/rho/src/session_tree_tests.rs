use super::persistence::{drop_incomplete_tool_turn_tail, resume_normalized_history};
use super::tree::{NodeId, SessionNode, SessionNodeKind, StoredStateTransition};
use super::*;
use rho_providers::model::AbortedAssistant;
use rho_tools::tool::ToolCall;

fn assert_mirrored_record_matches_file(session: &Session, mirrored: &SessionIndexRecord) {
    let reloaded = summarize_session_file(session.path(), session.cwd()).unwrap();
    pretty_assertions::assert_eq!(mirrored, &reloaded);
}

fn snapshot(
    session: &Session,
    revision: u64,
    history: Vec<Message>,
    compaction: CompactionState,
) -> SessionSnapshot {
    SessionSnapshot::new(
        SessionId::from_string(session.id().to_owned()).unwrap(),
        Revision::from_u64(revision),
        history,
        ModelIdentity::new("provider", "api", "model"),
        compaction,
    )
    .with_prompt_cache_key(format!("rho:{}", session.id()))
}

#[test]
fn v4_nodes_restore_declared_parent_and_support_branching() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let first = snapshot(
        &session,
        1,
        vec![Message::user_text("root")],
        CompactionState::default(),
    );
    session.save_snapshot(&first, first.history()).unwrap();
    let root_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    let left = snapshot(
        &session,
        2,
        vec![Message::user_text("root"), Message::assistant_text("left")],
        CompactionState::default(),
    );
    session.save_snapshot(&left, &left.history()[1..]).unwrap();
    let left_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    session.set_leaf(&root_id).unwrap();
    let right = snapshot(
        &session,
        2,
        vec![Message::user_text("root"), Message::assistant_text("right")],
        CompactionState::default(),
    );
    session
        .save_snapshot(&right, &right.history()[1..])
        .unwrap();

    let tree = session.session_tree().unwrap();
    let facts = session.tree_facts().unwrap();
    assert_eq!(facts.node_count, 3);
    assert_eq!(facts.branch_count, 1);
    assert_eq!(facts.active_leaf_id, tree.active_leaf_id().cloned());
    assert_eq!(tree.children(&root_id).len(), 2);
    assert!(tree.children(&root_id).contains(&left_id));
    assert_eq!(tree.active_path().unwrap().len(), 2);
    let root_node = tree.node(&root_id).unwrap();
    assert_eq!(root_node.kind(), SessionNodeKind::Commit);
    assert!(!root_node.timestamp().is_empty());
    assert_eq!(root_node.display_messages().len(), 1);
    assert_eq!(tree.active_state().unwrap().model, right.history());
    assert_eq!(
        summarize_session_file(session.path(), cwd.path())
            .unwrap()
            .summary
            .message_count,
        2
    );
    assert_eq!(
        session
            .snapshot_for_resume(
                ModelIdentity::new("unused", "unused", "unused"),
                "unused".into(),
            )
            .unwrap(),
        right
    );
}

// Covers: /tree restore must rebuild that node's own prefix, not the active leaf
// Owner: session tree
#[test]
fn state_for_rebuilds_each_nodes_own_history() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let first = snapshot(
        &session,
        1,
        vec![Message::user_text("root")],
        CompactionState::default(),
    );
    session.save_snapshot(&first, first.history()).unwrap();
    let first_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();
    let second = snapshot(
        &session,
        2,
        vec![Message::user_text("root"), Message::assistant_text("left")],
        CompactionState::default(),
    );
    session
        .save_snapshot(&second, &second.history()[1..])
        .unwrap();
    let second_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    let linear = session.session_tree().unwrap();
    pretty_assertions::assert_eq!(linear.state_for(&first_id).unwrap().model, first.history());
    pretty_assertions::assert_eq!(
        linear.state_for(&second_id).unwrap().model,
        second.history()
    );
    pretty_assertions::assert_eq!(linear.active_state().unwrap().model, second.history());

    session.set_leaf(&first_id).unwrap();
    let compaction =
        CompactionState::from_accounting(1, 0, 4, 0, Some(8), Some(4), Some(Revision::from_u64(3)));
    let compact = snapshot(&session, 3, vec![Message::user_text("summary")], compaction);
    session.save_snapshot(&compact, compact.history()).unwrap();
    let compact_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    let tree = session.session_tree().unwrap();
    let cases = [
        (&first_id, first.history()),
        (&second_id, second.history()),
        (&compact_id, compact.history()),
    ];
    for (id, history) in cases {
        pretty_assertions::assert_eq!(tree.state_for(id).unwrap().model, history);
    }
    pretty_assertions::assert_eq!(tree.active_state().unwrap().model, compact.history());
}

#[test]
fn compaction_state_change_writes_a_full_compaction_node() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let first = snapshot(
        &session,
        1,
        vec![Message::user_text("before")],
        CompactionState::default(),
    );
    session.save_snapshot(&first, &[]).unwrap();
    let compaction = CompactionState::from_accounting(
        1,
        1,
        10,
        0,
        Some(20),
        Some(10),
        Some(Revision::from_u64(2)),
    );
    let compacted = snapshot(&session, 2, vec![Message::user_text("summary")], compaction);
    session.save_snapshot(&compacted, &[]).unwrap();

    let entries = read_entries(session.path()).unwrap();
    assert!(matches!(
        entries.last(),
        Some(SessionEntry::Node {
            node: SessionNode {
                kind: SessionNodeKind::Compaction,
                compaction_facts: Some(facts),
                transition: StoredStateTransition::Snapshot { .. },
                ..
            }
        }) if facts.previous_messages == 1
            && facts.current_messages == 1
            && facts.previous_tokens == 20
            && facts.current_tokens == 10
    ));
}

#[test]
fn multiple_compactions_restore_in_ancestry_order() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let original = snapshot(
        &session,
        1,
        vec![Message::user_text("original")],
        CompactionState::default(),
    );
    session.save_snapshot(&original, &[]).unwrap();
    let original_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    let first_compaction = CompactionState::from_accounting(
        1,
        1,
        10,
        0,
        Some(20),
        Some(10),
        Some(Revision::from_u64(2)),
    );
    let first = snapshot(
        &session,
        2,
        vec![Message::user_text("summary one")],
        first_compaction,
    );
    session.save_snapshot(&first, &[]).unwrap();
    let first_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    let descendant = snapshot(
        &session,
        3,
        vec![
            Message::user_text("summary one"),
            Message::assistant_text("descendant"),
        ],
        first.compaction().clone(),
    );
    session.save_snapshot(&descendant, &[]).unwrap();
    let second_compaction = CompactionState::from_accounting(
        2,
        2,
        20,
        0,
        Some(30),
        Some(8),
        Some(Revision::from_u64(4)),
    );
    let second = snapshot(
        &session,
        4,
        vec![Message::user_text("summary two")],
        second_compaction,
    );
    session.save_snapshot(&second, &[]).unwrap();

    let tree = session.session_tree().unwrap();
    assert_eq!(
        tree.active_path()
            .unwrap()
            .iter()
            .map(|node| node.kind())
            .collect::<Vec<_>>(),
        vec![
            SessionNodeKind::Commit,
            SessionNodeKind::Compaction,
            SessionNodeKind::Commit,
            SessionNodeKind::Compaction,
        ]
    );
    assert_eq!(tree.active_state().unwrap().model, second.history());

    session.set_leaf(&original_id).unwrap();
    assert_eq!(
        session
            .session_tree()
            .unwrap()
            .active_state()
            .unwrap()
            .model,
        original.history()
    );
    session.set_leaf(&first_id).unwrap();
    assert_eq!(
        session
            .session_tree()
            .unwrap()
            .active_state()
            .unwrap()
            .model,
        first.history()
    );
}

#[test]
fn tree_items_use_depth_first_branch_order_and_connector_state() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let root_state = snapshot(
        &session,
        1,
        vec![
            Message::user_text("root"),
            Message::assistant_text("root reply"),
        ],
        CompactionState::default(),
    );
    session.save_snapshot(&root_state, &[]).unwrap();
    let root_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();
    let a_state = snapshot(
        &session,
        2,
        vec![
            Message::user_text("root"),
            Message::assistant_text("root reply"),
            Message::user_text("a"),
            Message::assistant_text("a reply"),
        ],
        CompactionState::default(),
    );
    session.save_snapshot(&a_state, &[]).unwrap();
    let a_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();
    session.set_leaf(&root_id).unwrap();
    let b_state = snapshot(
        &session,
        2,
        vec![
            Message::user_text("root"),
            Message::assistant_text("root reply"),
            Message::user_text("b"),
            Message::assistant_text("b reply"),
        ],
        CompactionState::default(),
    );
    session.save_snapshot(&b_state, &[]).unwrap();
    let b_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();
    session.set_leaf(&a_id).unwrap();
    let a1_state = snapshot(
        &session,
        3,
        vec![
            Message::user_text("root"),
            Message::assistant_text("root reply"),
            Message::user_text("a"),
            Message::assistant_text("a reply"),
            Message::user_text("a1"),
            Message::assistant_text("a1 reply"),
        ],
        CompactionState::default(),
    );
    session.save_snapshot(&a1_state, &[]).unwrap();
    let a1_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    let items = session.tree_items().unwrap();
    assert_eq!(
        items.iter().map(|item| item.id.clone()).collect::<Vec<_>>(),
        vec![root_id, a_id, a1_id, b_id]
    );
    assert!(!items[1].is_last_sibling);
    assert_eq!(items[2].ancestor_has_next_sibling, vec![true]);
    assert!(items[3].is_last_sibling);
}

#[test]
fn changed_history_prefix_uses_a_full_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let first = snapshot(
        &session,
        1,
        vec![
            Message::user_text("a"),
            Message::assistant_text("same tail"),
        ],
        CompactionState::default(),
    );
    session.save_snapshot(&first, &[]).unwrap();
    let changed = snapshot(
        &session,
        2,
        vec![
            Message::user_text("b"),
            Message::assistant_text("same tail"),
        ],
        CompactionState::default(),
    );
    session.save_snapshot(&changed, &[]).unwrap();

    assert!(matches!(
        read_entries(session.path()).unwrap().last(),
        Some(SessionEntry::Node {
            node: SessionNode {
                transition: StoredStateTransition::Snapshot { .. },
                ..
            }
        })
    ));
    assert_eq!(
        session
            .session_tree()
            .unwrap()
            .active_state()
            .unwrap()
            .model,
        changed.history()
    );
}

#[test]
fn loader_rejects_legacy_records_in_a_v4_log() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let entry = SessionEntry::Message {
        timestamp: "2".into(),
        message: Message::user_text("legacy record"),
        display_message: None,
    };
    let mut contents = fs::read_to_string(session.path()).unwrap();
    contents.push_str(&serde_json::to_string(&entry).unwrap());
    contents.push('\n');
    fs::write(session.path(), contents).unwrap();

    let error = session.session_tree().unwrap_err();
    assert!(error
        .to_string()
        .contains("legacy record appears in the explicit tree phase"));
}

#[test]
fn loader_rejects_invalid_explicit_node_ids() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let state = snapshot(
        &session,
        1,
        vec![Message::user_text("root")],
        CompactionState::default(),
    );
    session.save_snapshot(&state, &[]).unwrap();
    let mut lines = fs::read_to_string(session.path())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut node: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    node["id"] = serde_json::Value::String(String::new());
    *lines.last_mut().unwrap() = serde_json::to_string(&node).unwrap();
    fs::write(session.path(), format!("{}\n", lines.join("\n"))).unwrap();

    let error = session.session_tree().unwrap_err();
    assert!(error
        .to_string()
        .contains("session node id cannot be empty"));
}

#[test]
fn loader_rejects_a_second_explicit_root() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let first = snapshot(
        &session,
        1,
        vec![Message::assistant_text("first")],
        CompactionState::default(),
    );
    session.save_snapshot(&first, &[]).unwrap();
    let second = snapshot(
        &session,
        2,
        vec![Message::assistant_text("second")],
        CompactionState::default(),
    );
    session.save_snapshot(&second, &[]).unwrap();
    let mut lines = fs::read_to_string(session.path())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut node: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    node.as_object_mut().unwrap().remove("parent_id");
    *lines.last_mut().unwrap() = serde_json::to_string(&node).unwrap();
    fs::write(session.path(), format!("{}\n", lines.join("\n"))).unwrap();

    let error = session.session_tree().unwrap_err();
    assert!(error.to_string().contains("disconnected root"));
}

#[test]
fn loader_rejects_changed_state_without_revision_advance() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let first = snapshot(
        &session,
        1,
        vec![Message::assistant_text("first")],
        CompactionState::default(),
    );
    session.save_snapshot(&first, &[]).unwrap();
    let second = snapshot(
        &session,
        2,
        vec![Message::assistant_text("changed")],
        CompactionState::default(),
    );
    session.save_snapshot(&second, &[]).unwrap();
    let mut lines = fs::read_to_string(session.path())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut node: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    node["transition"]["snapshot"]["revision"] = serde_json::Value::from(1);
    *lines.last_mut().unwrap() = serde_json::to_string(&node).unwrap();
    fs::write(session.path(), format!("{}\n", lines.join("\n"))).unwrap();

    let error = session.session_tree().unwrap_err();
    assert!(
        error.to_string().contains("revision"),
        "unexpected error: {error}"
    );
}

#[test]
fn legacy_projection_uses_stable_byte_offset_ids_and_one_upgrade_marker() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let id = "33333333-3333-4333-8333-333333333333";
    let dir = session_dir_in_root(root.path(), cwd.path());
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("1_{id}.jsonl"));
    let fixture = include_str!("session/fixtures/session-v3.jsonl");
    let mut lines = fixture.lines();
    let mut header = serde_json::from_str::<serde_json::Value>(lines.next().unwrap()).unwrap();
    header["cwd"] = serde_json::Value::String(cwd.path().to_string_lossy().into_owned());
    let transcript = std::iter::once(header.to_string())
        .chain(lines.map(str::to_owned))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&path, format!("{transcript}\n")).unwrap();
    let (session, _) = Session::open_by_id_in_root(root.path(), cwd.path(), id).unwrap();

    let first_ids = session
        .session_tree()
        .unwrap()
        .nodes_in_storage_order()
        .map(|node| node.id().clone())
        .collect::<Vec<_>>();
    let second_ids = session
        .session_tree()
        .unwrap()
        .nodes_in_storage_order()
        .map(|node| node.id().clone())
        .collect::<Vec<_>>();
    assert_eq!(first_ids, second_ids);
    assert!(first_ids
        .iter()
        .all(|id| id.as_str().starts_with("legacy:")));

    let resumed = session
        .snapshot_for_resume(
            ModelIdentity::new("unused", "unused", "unused"),
            "unused".into(),
        )
        .unwrap();
    session.save_snapshot(&resumed, &[]).unwrap();
    session.save_snapshot(&resumed, &[]).unwrap();
    let entries = read_entries(session.path()).unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, SessionEntry::Upgrade { .. }))
            .count(),
        1
    );
}

#[test]
fn loader_rejects_missing_declared_parent_without_using_file_order() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let first = snapshot(
        &session,
        1,
        vec![Message::user_text("root")],
        CompactionState::default(),
    );
    let second = snapshot(
        &session,
        2,
        vec![Message::user_text("root"), Message::assistant_text("tail")],
        CompactionState::default(),
    );
    session.save_snapshot(&first, &[]).unwrap();
    session.save_snapshot(&second, &[]).unwrap();

    let error = session
        .set_leaf(&NodeId::from_string("missing-parent").unwrap())
        .unwrap_err();
    assert!(error.to_string().contains("missing session node"));

    let mut lines = fs::read_to_string(session.path())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut node: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    node["parent_id"] = serde_json::Value::String("missing-parent".into());
    *lines.last_mut().unwrap() = serde_json::to_string(&node).unwrap();
    fs::write(session.path(), format!("{}\n", lines.join("\n"))).unwrap();

    let error = session.session_tree().unwrap_err();
    assert!(error.to_string().contains("missing parent"), "{error:#}");
}

#[test]
fn truncated_set_leaf_does_not_change_the_active_leaf() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let state = snapshot(
        &session,
        1,
        vec![Message::user_text("durable")],
        CompactionState::default(),
    );
    session.save_snapshot(&state, &[]).unwrap();
    let active = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();
    OpenOptions::new()
        .append(true)
        .open(session.path())
        .unwrap()
        .write_all(b"{\"type\":\"set_leaf\",\"timestamp\":\"2\"")
        .unwrap();

    assert_eq!(
        session.session_tree().unwrap().active_leaf_id(),
        Some(&active)
    );
}

// Covers: the record built from the mutated tree after save/set_leaf must match a file reload
// Owner: session persistence
#[test]
fn mirrored_tree_index_record_equals_reloaded_file_summary() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();

    // Turn 1: Initial user message and assistant reply
    let first = snapshot(
        &session,
        1,
        vec![
            Message::user_text("first question"),
            Message::assistant_text("first answer"),
        ],
        CompactionState::default(),
    );
    let first_record = session
        .save_snapshot_mirrored_record(&first, first.history())
        .unwrap();
    assert_mirrored_record_matches_file(&session, &first_record);
    let first_leaf_id = NodeId::from_string(first_record.active_leaf_id.clone().unwrap()).unwrap();

    // Turn 2: Follow-up turn
    let second = snapshot(
        &session,
        2,
        vec![
            Message::user_text("first question"),
            Message::assistant_text("first answer"),
            Message::user_text("second question"),
            Message::assistant_text("second answer"),
        ],
        CompactionState::default(),
    );
    let second_record = session
        .save_snapshot_mirrored_record(&second, &second.history()[2..])
        .unwrap();
    assert_mirrored_record_matches_file(&session, &second_record);

    // Turn 3: Compaction turn
    let compaction_state = CompactionState::from_accounting(
        1,
        2,
        100,
        0,
        Some(150),
        Some(50),
        Some(Revision::from_u64(3)),
    );
    let third = snapshot(
        &session,
        3,
        vec![
            Message::user_text("summary of earlier turns"),
            Message::user_text("third question"),
        ],
        compaction_state,
    );
    let third_record = session
        .save_snapshot_mirrored_record(&third, &third.history()[1..])
        .unwrap();
    assert_mirrored_record_matches_file(&session, &third_record);

    // Turn 4: set_leaf back to first turn (branching)
    let branched_record = session.set_leaf_mirrored_record(&first_leaf_id).unwrap();
    assert_mirrored_record_matches_file(&session, &branched_record);

    // Turn 5: New turn on the branch
    let branched = snapshot(
        &session,
        4,
        vec![
            Message::user_text("first question"),
            Message::assistant_text("first answer"),
            Message::user_text("alternate branch question"),
            Message::assistant_text("alternate answer"),
        ],
        CompactionState::default(),
    );
    let after_branch_record = session
        .save_snapshot_mirrored_record(&branched, &branched.history()[2..])
        .unwrap();
    assert_mirrored_record_matches_file(&session, &after_branch_record);
}

// Covers: upgrade-marker save and set_leaf on a v3 transcript must mirror the file
// Owner: session persistence
#[test]
fn mirrored_tree_legacy_upgrade_index_record_equals_reloaded_file_summary() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let id = "33333333-3333-4333-8333-333333333333";
    let dir = session_dir_in_root(root.path(), cwd.path());
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("1_{id}.jsonl"));
    let fixture = include_str!("session/fixtures/session-v3.jsonl");
    let mut lines = fixture.lines();
    let mut header = serde_json::from_str::<serde_json::Value>(lines.next().unwrap()).unwrap();
    header["cwd"] = serde_json::Value::String(cwd.path().to_string_lossy().into_owned());
    let transcript = std::iter::once(header.to_string())
        .chain(lines.map(str::to_owned))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&path, format!("{transcript}\n")).unwrap();

    let (session, _) = Session::open_by_id_in_root(root.path(), cwd.path(), id).unwrap();
    let legacy_leaf_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    // Save a new turn triggering the upgrade marker path
    let resumed = session
        .snapshot_for_resume(
            ModelIdentity::new("provider", "api", "model"),
            "prompt-key".into(),
        )
        .unwrap();
    let mut history = resumed.history().to_vec();
    history.push(Message::user_text("new turn after upgrade"));
    let upgraded_snapshot = SessionSnapshot::new(
        SessionId::from_string(session.id().to_owned()).unwrap(),
        Revision::from_u64(resumed.revision().get() + 1),
        history.clone(),
        resumed.provider().clone(),
        resumed.compaction().clone(),
    );
    let upgraded_record = session
        .save_snapshot_mirrored_record(
            &upgraded_snapshot,
            &[Message::user_text("new turn after upgrade")],
        )
        .unwrap();
    assert_mirrored_record_matches_file(&session, &upgraded_record);

    let branched_record = session.set_leaf_mirrored_record(&legacy_leaf_id).unwrap();
    assert_mirrored_record_matches_file(&session, &branched_record);
}

// Covers: attaching to a parentless v1 virtual leaf and set_leaf must mirror the file
// Owner: session persistence
#[test]
fn mirrored_tree_legacy_v1_no_parent_snapshot_upgrade_index_record_equals_reloaded_file_summary() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let id = "11111111-1111-4111-8111-111111111111";
    let dir = session_dir_in_root(root.path(), cwd.path());
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("1_{id}.jsonl"));
    let fixture = include_str!("session/fixtures/session-v1.jsonl");
    let mut lines = fixture.lines();
    let mut header = serde_json::from_str::<serde_json::Value>(lines.next().unwrap()).unwrap();
    header["cwd"] = serde_json::Value::String(cwd.path().to_string_lossy().into_owned());
    let transcript = std::iter::once(header.to_string())
        .chain(lines.map(str::to_owned))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&path, format!("{transcript}\n")).unwrap();

    let (session, _) = Session::open_by_id_in_root(root.path(), cwd.path(), id).unwrap();
    let legacy_leaf_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    // Save a new turn attaching an explicit node to the legacy virtual leaf with no parent snapshot
    let resumed = session
        .snapshot_for_resume(
            ModelIdentity::new("provider", "api", "model"),
            "prompt-key".into(),
        )
        .unwrap();
    let mut history = resumed.history().to_vec();
    history.push(Message::user_text("new turn after v1 upgrade"));
    let upgraded_snapshot = SessionSnapshot::new(
        SessionId::from_string(session.id().to_owned()).unwrap(),
        Revision::from_u64(resumed.revision().get() + 1),
        history.clone(),
        resumed.provider().clone(),
        resumed.compaction().clone(),
    );
    let upgraded_record = session
        .save_snapshot_mirrored_record(
            &upgraded_snapshot,
            &[Message::user_text("new turn after v1 upgrade")],
        )
        .unwrap();
    assert_mirrored_record_matches_file(&session, &upgraded_record);

    let branched_record = session.set_leaf_mirrored_record(&legacy_leaf_id).unwrap();
    assert_mirrored_record_matches_file(&session, &branched_record);
}

fn open_v1_session_with_tail(extra: &[Message]) -> (tempfile::TempDir, tempfile::TempDir, Session) {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let id = "11111111-1111-4111-8111-111111111111";
    let dir = session_dir_in_root(root.path(), cwd.path());
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("1_{id}.jsonl"));
    let fixture = include_str!("session/fixtures/session-v1.jsonl");
    let mut lines = fixture.lines();
    let mut header = serde_json::from_str::<serde_json::Value>(lines.next().unwrap()).unwrap();
    header["cwd"] = serde_json::Value::String(cwd.path().to_string_lossy().into_owned());
    let mut transcript = std::iter::once(header.to_string())
        .chain(lines.map(str::to_owned))
        .collect::<Vec<_>>();
    for (offset, message) in extra.iter().enumerate() {
        transcript.push(
            serde_json::to_string(&SessionEntry::Message {
                timestamp: (200 + offset).to_string(),
                message: message.clone(),
                display_message: None,
            })
            .unwrap(),
        );
    }
    fs::write(&path, format!("{}\n", transcript.join("\n"))).unwrap();
    let (session, _) = Session::open_by_id_in_root(root.path(), cwd.path(), id).unwrap();
    (root, cwd, session)
}

fn incomplete_tool_tail() -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: "call-1".into(),
        name: "bash".into(),
        arguments: serde_json::json!({"command": "echo hi"}),
    })])
}

fn aborted_assistant_with_reasoning() -> Message {
    Message::AbortedAssistant(Box::new(AbortedAssistant {
        content: vec![ContentBlock::Text("partial".into())],
        reasoning: "cleared-on-resume".into(),
        ..AbortedAssistant::default()
    }))
}

// Covers: local resume normalizer must match drop_incomplete plus SDK sanitize_history
// Owner: session persistence
#[test]
fn resume_normalized_history_matches_sdk_sanitize_history() {
    let cases = [
        Vec::new(),
        vec![Message::user_text("hello")],
        vec![incomplete_tool_tail()],
        vec![aborted_assistant_with_reasoning()],
        vec![
            Message::user_text("hello"),
            aborted_assistant_with_reasoning(),
            incomplete_tool_tail(),
        ],
    ];
    for history in cases {
        pretty_assertions::assert_eq!(
            resume_normalized_history(history.clone()),
            SessionSnapshot::sanitize_history(drop_incomplete_tool_turn_tail(history))
        );
    }
}

// Covers: v1 same-revision upgrade must stay loadable and index the new leaf
// Owner: session persistence
#[test]
fn v1_same_revision_upgrade_stays_loadable_and_indexes_the_new_leaf() {
    let cases = [
        ("fixture", Vec::new()),
        ("incomplete tool tail", vec![incomplete_tool_tail()]),
        (
            "aborted assistant reasoning",
            vec![aborted_assistant_with_reasoning()],
        ),
    ];
    for (name, extra) in cases {
        let (_root, _cwd, session) = open_v1_session_with_tail(&extra);
        let before = session.session_tree().unwrap();
        let before_leaf = before.active_leaf_id().unwrap().clone();
        let before_nodes = before.facts().node_count;
        let parent_model = before.active_state().unwrap().model.clone();

        let snapshot = session
            .snapshot_for_resume(
                ModelIdentity::new("target", "api", "model"),
                "rho:migrated-v1".into(),
            )
            .unwrap();
        if !extra.is_empty() {
            assert_ne!(
                snapshot.history(),
                parent_model.as_slice(),
                "{name}: resume must normalize history so this case stays distinct"
            );
        }
        let mirrored = session
            .save_snapshot_mirrored_record(&snapshot, &[])
            .unwrap();
        assert_mirrored_record_matches_file(&session, &mirrored);

        let tree = session.session_tree().unwrap();
        let facts = tree.facts();
        assert_eq!(facts.node_count, before_nodes + 1, "{name}");
        assert_ne!(facts.active_leaf_id.as_ref(), Some(&before_leaf), "{name}");
        assert_eq!(
            facts.active_leaf_id.map(|id| id.to_string()),
            mirrored.active_leaf_id,
            "{name}"
        );
    }
}

// Covers: a rejected same-revision v1 state change must not leave an unloadable transcript
// Owner: session persistence
#[test]
fn rejected_v1_same_revision_state_change_leaves_transcript_loadable() {
    let (_root, _cwd, session) = open_v1_session_with_tail(&[]);
    let before = session.session_tree().unwrap();
    let before_leaf = before.active_leaf_id().unwrap().clone();
    let before_nodes = before.facts().node_count;
    let resumed = session
        .snapshot_for_resume(
            ModelIdentity::new("target", "api", "model"),
            "rho:migrated-v1".into(),
        )
        .unwrap();
    let mut history = resumed.history().to_vec();
    history.push(Message::user_text("unsaved extra turn"));
    let changed = SessionSnapshot::new(
        resumed.session_id().clone(),
        resumed.revision(),
        history,
        resumed.provider().clone(),
        resumed.compaction().clone(),
    )
    .with_prompt_cache_key(resumed.prompt_cache_key().unwrap_or("rho:migrated-v1"));

    assert!(session.save_snapshot(&changed, &[]).is_err());

    let after = session.session_tree().unwrap();
    assert_eq!(after.facts().node_count, before_nodes);
    assert_eq!(after.active_leaf_id(), Some(&before_leaf));
}
