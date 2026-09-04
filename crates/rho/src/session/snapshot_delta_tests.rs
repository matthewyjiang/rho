use super::*;
use pretty_assertions::assert_eq;
use rho_providers::model::{AbortedAssistant, ContentBlock};

// Covers: delta replay must clear raw aborted reasoning before materializing a snapshot.
// Owner: session persistence; snapshot construction alone would hide this regression.
#[test]
fn replay_sanitizes_appended_history_like_sdk_snapshots() {
    let previous = SessionSnapshot::new(
        SessionId::new(),
        Revision::from_u64(1),
        vec![Message::user_text("existing")],
        ModelIdentity::new("provider", "api", "model"),
        CompactionState::default(),
    );
    let appended_history = vec![
        Message::user_text("next"),
        Message::AbortedAssistant(Box::new(AbortedAssistant {
            content: vec![ContentBlock::Text("partial answer".into())],
            reasoning: "private reasoning".into(),
            ..AbortedAssistant::default()
        })),
        Message::Assistant(vec![ContentBlock::Text("complete answer".into())]),
    ];
    let mut expected = previous.history().to_vec();
    expected.extend(SessionSnapshot::sanitize_history(appended_history.clone()));
    let delta = StoredSnapshotDelta {
        base_revision: previous.revision(),
        session_id: previous.session_id().clone(),
        revision: Revision::from_u64(2),
        appended_history,
        provider: previous.provider().clone(),
        compaction: previous.compaction().clone(),
        metadata: previous.metadata().clone(),
        prompt_cache_key: None,
    };
    let mut replay = SnapshotReplay::new(&previous, previous.history().to_vec());
    assert!(delta.apply(&mut replay).unwrap());
    assert_eq!(replay.history, expected);
}
