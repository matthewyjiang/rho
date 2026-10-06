use std::fs::OpenOptions;
use std::io::Write;

use pretty_assertions::assert_eq;
use rho_providers::model::{Message, ModelIdentity};
use rho_sdk::{CompactionState, Revision, SessionId, SessionSnapshot};

use super::super::tree::{SessionTree, SessionTreeItemKind};
use super::super::Session;

fn snapshot(session: &Session, revision: u64, history: Vec<Message>) -> SessionSnapshot {
    SessionSnapshot::new(
        SessionId::from_string(session.id().to_owned()).unwrap(),
        Revision::from_u64(revision),
        history,
        ModelIdentity::new("provider", "api", "model"),
        CompactionState::default(),
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
    // Missing user text alone must not classify later commits as session start.
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
    let items = tree.items()?;
    assert_eq!(
        items.iter().map(|item| item.kind).collect::<Vec<_>>(),
        vec![
            SessionTreeItemKind::SessionStart,
            SessionTreeItemKind::Turn,
            SessionTreeItemKind::Turn,
        ]
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

// Covers: a second save must parent on the in-memory tree, matching a fresh
// reload, without depending on a stale disk parse.
// Owner: session persistence
#[test]
fn sequential_saves_match_fresh_reload() {
    let (_root, _cwd, session) = create_session();
    let first = snapshot(&session, 1, vec![Message::user_text("root")]);
    session.save_snapshot(&first, first.history()).unwrap();
    let second = snapshot(
        &session,
        2,
        vec![Message::user_text("root"), Message::assistant_text("tail")],
    );
    session
        .save_snapshot(
            &second,
            std::slice::from_ref(second.history().last().unwrap()),
        )
        .unwrap();

    let cached = session.session_tree().unwrap();
    let reloaded = SessionTree::load(session.path()).unwrap();
    assert_eq!(cached.facts(), reloaded.facts());
    assert_eq!(
        cached.active_state().map(|state| state.model.clone()),
        reloaded.active_state().map(|state| state.model.clone())
    );
    assert_eq!(
        session
            .snapshot_for_resume(
                ModelIdentity::new("unused", "unused", "unused"),
                "unused".into(),
            )
            .unwrap(),
        second
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
