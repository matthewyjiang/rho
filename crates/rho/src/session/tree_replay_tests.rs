use std::fs;

use rho_providers::model::{Message, ModelIdentity};
use rho_sdk::{CompactionState, Revision, SessionId, SessionSnapshot};
use serde_json::{json, Value};

use crate::session::{tree::SessionTree, Session};

// Covers: deferred replay must not accept changed state at an unchanged revision,
// or skip base/session/compaction validation after the first delta.
// Owner: session tree loader. Existing full-snapshot checks do not exercise this.
#[test]
fn replay_validates_every_delta_even_when_revision_is_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let first = SessionSnapshot::new(
        SessionId::from_string(session.id().to_owned()).unwrap(),
        Revision::from_u64(1),
        vec![Message::user_text("root")],
        ModelIdentity::new("provider", "api", "model"),
        CompactionState::default(),
    );
    let second = SessionSnapshot::new(
        first.session_id().clone(),
        Revision::from_u64(2),
        vec![Message::user_text("root"), Message::assistant_text("reply")],
        first.provider().clone(),
        CompactionState::default(),
    )
    .with_metadata("context", "kept")
    .with_prompt_cache_key("stable-key");
    session.save_snapshot(&first, first.history()).unwrap();
    session
        .save_snapshot(&second, &second.history()[1..])
        .unwrap();
    session.save_snapshot(&second, &[]).unwrap();
    let tree = SessionTree::load(session.path()).unwrap();
    pretty_assertions::assert_eq!(
        tree.active_state().unwrap().snapshot.as_ref(),
        Some(&second)
    );
    pretty_assertions::assert_eq!(tree.active_state().unwrap().model, second.history());

    let text = fs::read_to_string(session.path()).unwrap();
    let mut lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let original = lines.last().unwrap().clone();
    let cases = [
        ("metadata", json!({"context": "changed"})),
        ("prompt_cache_key", json!("changed-key")),
        (
            "provider",
            serde_json::to_value(ModelIdentity::new("other", "api", "model")).unwrap(),
        ),
        (
            "appended_history",
            serde_json::to_value(vec![Message::user_text("unexpected")]).unwrap(),
        ),
        ("session_id", json!("wrong-session")),
        ("base_revision", json!(1)),
        ("revision", json!(1)),
        (
            "compaction",
            serde_json::to_value(CompactionState::from_accounting(
                1,
                0,
                4,
                0,
                Some(8),
                Some(4),
                Some(Revision::from_u64(3)),
            ))
            .unwrap(),
        ),
    ];
    for (field, value) in cases {
        let mut node = original.clone();
        node["transition"]["delta"][field] = value;
        *lines.last_mut().unwrap() = node;
        let contents = lines
            .iter()
            .map(|line| serde_json::to_string(line).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(session.path(), format!("{contents}\n")).unwrap();
        assert!(
            SessionTree::load(session.path()).is_err(),
            "accepted corrupt delta field {field}"
        );
    }
}
