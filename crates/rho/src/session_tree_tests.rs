use super::tree::{NodeId, SessionNodeKind};
use super::*;
use std::fs::OpenOptions;

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
    let compaction = CompactionState::from_parts(1, 0, Some(Revision::from_u64(3)))
        .with_removed_tokens(4)
        .with_last_compaction_tokens(8, 4);
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
        let state = tree.state_for(id).unwrap();
        pretty_assertions::assert_eq!(state.model, history);
        assert!(
            state.snapshot.is_some(),
            "explicit v4 nodes keep a real snapshot"
        );
    }
    pretty_assertions::assert_eq!(tree.active_state().unwrap().model, compact.history());
}

// Covers: a tampered explicit tree log fails to load, naming the broken rule,
// instead of guessing from file order.
// Owner: session tree loader
#[test]
fn loader_rejects_tampered_tree_records() {
    fn edit_last_node(lines: &mut [String], edit: impl FnOnce(&mut serde_json::Value)) {
        let mut node: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        edit(&mut node);
        *lines.last_mut().unwrap() = serde_json::to_string(&node).unwrap();
    }
    /// Shape of the second saved snapshot: `Full` is not a prefix extension,
    /// so it is stored whole (the "revision" case rewrites its revision);
    /// `Delta` extends the first, so it is stored as a delta whose parent link
    /// the "missing parent" case breaks.
    #[derive(Clone, Copy)]
    enum Tail {
        Full,
        Delta,
    }
    type Tamper = fn(&mut Vec<String>);
    let cases: [(&str, Tail, Tamper); 4] = [
        ("session node id cannot be empty", Tail::Full, |lines| {
            edit_last_node(lines, |node| node["id"] = String::new().into());
        }),
        ("disconnected root", Tail::Full, |lines| {
            edit_last_node(lines, |node| {
                node.as_object_mut().unwrap().remove("parent_id");
            });
        }),
        ("revision", Tail::Full, |lines| {
            edit_last_node(lines, |node| {
                node["transition"]["snapshot"]["revision"] = 1.into();
            });
        }),
        ("missing parent", Tail::Delta, |lines| {
            edit_last_node(lines, |node| node["parent_id"] = "missing-parent".into());
        }),
    ];
    for (expected, tail, tamper) in cases {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
        let first = snapshot(
            &session,
            1,
            vec![Message::user_text("root")],
            CompactionState::default(),
        );
        let second_messages = match tail {
            Tail::Full => vec![Message::assistant_text("changed")],
            Tail::Delta => vec![Message::user_text("root"), Message::assistant_text("tail")],
        };
        let second = snapshot(&session, 2, second_messages, CompactionState::default());
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
        tamper(&mut lines);
        fs::write(session.path(), format!("{}\n", lines.join("\n"))).unwrap();

        let error = session.session_tree().unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "{expected}: {error:#}"
        );
    }
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
    let compaction_state = CompactionState::from_parts(1, 2, Some(Revision::from_u64(3)))
        .with_removed_tokens(100)
        .with_last_compaction_tokens(150, 50);
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
