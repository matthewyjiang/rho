use super::tree::{NodeId, SessionNode, SessionNodeKind, StoredStateTransition};
use super::*;

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
    fs::write(&path, include_str!("session/fixtures/session-v3.jsonl")).unwrap();
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
