use std::fs::OpenOptions;
use std::io::Write;

use pretty_assertions::assert_eq;
use rho_providers::model::{Message, ModelIdentity};
use rho_sdk::{CompactionState, Revision, SessionId, SessionSnapshot};

use super::super::tree::{
    SessionNodeKind, SessionTree, SessionTreeItemKind, StoredStateTransition,
};
use super::super::Session;

fn snapshot(session: &Session, revision: u64, history: Vec<Message>) -> SessionSnapshot {
    compacted_snapshot(session, revision, history, CompactionState::default())
}

fn compacted_snapshot(
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

fn create_session() -> (tempfile::TempDir, tempfile::TempDir, Session) {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    (root, cwd, session)
}

// Covers: the durable pre-turn baseline is a session start, not a phantom turn.
// Owner: session tree projection; assert classification, not picker label copy.
#[test]
fn empty_root_is_session_start_and_remains_restorable() -> anyhow::Result<()> {
    let (_root, _cwd, session) = create_session();
    let baseline = snapshot(&session, 1, Vec::new());
    session.save_snapshot(&baseline, &[])?;
    let baseline_id = session.active_checkpoint_target()?.unwrap().0;
    let first_turn = snapshot(&session, 2, vec![Message::user_text("first prompt")]);
    session.save_snapshot(&first_turn, first_turn.history())?;
    // Missing user text alone must not classify a later commit as a session
    // start: it carries the turn before it on.
    let assistant_turn = snapshot(
        &session,
        3,
        vec![
            Message::user_text("first prompt"),
            Message::assistant_text("reply"),
        ],
    );
    session.save_snapshot(&assistant_turn, &assistant_turn.history()[1..])?;

    let tree = SessionTree::load(session.path())?;
    let items = tree.turn_items()?;
    assert_eq!(
        items.iter().map(|item| item.kind).collect::<Vec<_>>(),
        vec![SessionTreeItemKind::SessionStart, SessionTreeItemKind::Turn]
    );
    assert_eq!(items[0].id, baseline_id);
    assert_eq!(items[0].first_user_text, None);
    session.set_leaf(&baseline_id)?;
    assert_eq!(
        session.snapshot_for_resume(baseline.provider().clone(), "unused".into())?,
        baseline
    );
    assert!(session.histories_for_node(&baseline_id)?.display.is_empty());
    Ok(())
}

// Covers: `/tree` lists one row for each prompt however many nodes its turn
// saved: the row names the turn's last node, so restoring it gives the
// conversation once the turn ended, a branch from that node starts a row of
// its own, and the row holding the active leaf is the active one; a node
// names the prompt of the turn that saved it.
// Owner: session tree projection.
#[test]
fn turn_rows_join_the_nodes_that_carry_a_prompt_on() -> anyhow::Result<()> {
    let (_root, _cwd, session) = create_session();
    let system = || Message::System("prompt".into());
    let saved = |revision: u64, history: Vec<Message>, shown: usize| {
        let snapshot = snapshot(&session, revision, history);
        let rows = &snapshot.history()[snapshot.history().len() - shown..];
        session.save_snapshot_node(&snapshot, rows)
    };
    let user = Message::user_text;
    let reply = Message::assistant_text;
    // A host that saves before the first prompt, then at each step.
    saved(1, vec![system()], 0)?;
    saved(2, vec![system(), user("first")], 1)?;
    let first_end = saved(3, vec![system(), user("first"), reply("one")], 1)?;
    saved(
        4,
        vec![system(), user("first"), reply("one"), user("second")],
        1,
    )?;
    let second = [system(), user("first"), reply("one"), user("second")];
    let second_end = saved(5, [&second[..], &[reply("two")]].concat(), 1)?;
    // Back to the first turn's end, then a prompt that is still running.
    session.set_leaf(&first_end)?;
    let branch = saved(
        6,
        vec![system(), user("first"), reply("one"), user("other")],
        1,
    )?;

    let tree = SessionTree::load(session.path())?;
    let rows = tree
        .turn_items()?
        .into_iter()
        .map(|item| {
            (
                item.kind,
                item.first_user_text,
                item.id,
                item.depth,
                item.active,
            )
        })
        .collect::<Vec<_>>();
    // A node a turn saved after its prompt still names that prompt.
    assert_eq!(
        [&first_end, &second_end, &branch].map(|node| tree.turn_prompt(node)),
        ["first", "second", "other"].map(|prompt| Some(prompt.to_owned()))
    );
    let turn = |text: &str| (SessionTreeItemKind::Turn, Some(text.to_owned()));
    let row = |(kind, text): (SessionTreeItemKind, Option<String>),
               id: &crate::session::tree::NodeId,
               depth,
               active| { (kind, text, id.clone(), depth, active) };
    assert_eq!(
        rows[1..],
        [
            row(turn("first"), &first_end, 1, false),
            row(turn("second"), &second_end, 2, false),
            row(turn("other"), &branch, 2, true),
        ]
    );
    assert_eq!(
        (rows.len(), rows[0].0, rows[0].3),
        (4, SessionTreeItemKind::SessionStart, 0)
    );
    Ok(())
}

// Covers: saves leave the cached active state as a pending replay. Deltas,
// rewritten prefixes, and compactions built on it, and readers after it, must
// match a fresh reload, without depending on a stale disk parse.
// Owner: session persistence
#[test]
fn sequential_saves_match_fresh_reload() {
    let (_root, _cwd, session) = create_session();
    let compacted = CompactionState::from_parts(1, 3, Some(Revision::from_u64(6)))
        .with_removed_tokens(30)
        .with_last_compaction_tokens(40, 10);
    let at = |revision: u64, history: &[&str], compaction: &CompactionState| {
        let history = history.iter().copied().map(Message::user_text).collect();
        compacted_snapshot(&session, revision, history, compaction.clone())
    };
    let resume = || {
        session
            .snapshot_for_resume(
                ModelIdentity::new("unused", "unused", "unused"),
                "unused".into(),
            )
            .unwrap()
    };
    let none = CompactionState::default();
    let saves = [
        at(1, &["u1"], &none),
        at(2, &["u1", "a1"], &none),
        at(3, &["u1", "a1", "u2"], &none),
        at(4, &["u1", "a1", "u2", "a2"], &none),
        at(5, &["u1", "a1", "u2", "a2", "u3"], &none),
        at(6, &["summary"], &compacted),
        at(7, &["summary", "a3"], &compacted),
        at(8, &["edited", "a3", "u4"], &compacted),
    ];
    for (index, snapshot) in saves.iter().enumerate() {
        session.save_snapshot(snapshot, &[]).unwrap();
        // A mid-chain read must leave the pending replay intact.
        if index == 2 {
            assert_eq!(resume(), *snapshot);
        }
    }

    let cached = session.session_tree().unwrap();
    let reloaded = SessionTree::load(session.path()).unwrap();
    assert_eq!(cached.facts(), reloaded.facts());
    assert_eq!(
        cached.active_state().map(|state| state.model.clone()),
        reloaded.active_state().map(|state| state.model.clone())
    );
    assert_eq!(resume(), saves[saves.len() - 1]);
    let transitions: Vec<_> = reloaded
        .nodes_in_storage_order()
        .map(|node| {
            let stored = match node.transition() {
                StoredStateTransition::Snapshot { .. } => "snapshot",
                StoredStateTransition::SnapshotDelta { .. } => "delta",
            };
            let previous_messages = node.compaction_facts().map(|facts| facts.previous_messages);
            (node.kind(), stored, previous_messages)
        })
        .collect();
    assert_eq!(
        transitions,
        [
            (SessionNodeKind::Commit, "snapshot", None),
            (SessionNodeKind::Commit, "delta", None),
            (SessionNodeKind::Commit, "delta", None),
            (SessionNodeKind::Commit, "delta", None),
            (SessionNodeKind::Commit, "delta", None),
            (SessionNodeKind::Compaction, "snapshot", Some(5)),
            (SessionNodeKind::Commit, "delta", None),
            (SessionNodeKind::Commit, "snapshot", None),
        ]
    );
}

// Covers: an external append that grows the transcript must not be ignored by
// the cached tree on the next save.
// Owner: session persistence
#[test]
fn external_file_growth_is_visible_on_next_save() {
    let (_root, _cwd, session) = create_session();
    let first = snapshot(&session, 1, vec![Message::user_text("root")]);
    session.save_snapshot(&first, first.history()).unwrap();
    let root_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    let sibling = snapshot(
        &session,
        2,
        vec![Message::user_text("root"), Message::assistant_text("side")],
    );
    session
        .save_snapshot(
            &sibling,
            std::slice::from_ref(sibling.history().last().unwrap()),
        )
        .unwrap();
    session.set_leaf(&root_id).unwrap();

    OpenOptions::new()
        .append(true)
        .open(session.path())
        .unwrap()
        .write_all(
            b"{\"type\":\"set_leaf\",\"timestamp\":\"1\",\"target_id\":\"missing-external\"}\n",
        )
        .unwrap();

    let error = session
        .save_snapshot(
            &snapshot(
                &session,
                3,
                vec![Message::user_text("root"), Message::assistant_text("next")],
            ),
            &[Message::assistant_text("next")],
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("missing"),
        "cached tree ignored external growth: {error:#}"
    );
}
