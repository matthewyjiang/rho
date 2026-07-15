use std::num::{NonZeroU64, NonZeroUsize};

use pretty_assertions::assert_eq;

use crate::{
    model::Message, CompactionOutput, CompactionPolicy, CompactionRequest, Compactor,
    ScriptedCompactor,
};

#[test]
fn policy_uses_explicit_message_threshold() {
    let policy = CompactionPolicy::after_messages(NonZeroUsize::new(3).unwrap());

    assert!(!policy.should_compact(2, u64::MAX));
    assert!(policy.should_compact(3, 0));
}

#[test]
fn policy_uses_explicit_context_token_threshold() {
    let policy = CompactionPolicy::at_context_tokens(NonZeroU64::new(1_000).unwrap());

    assert!(!policy.should_compact(usize::MAX, 999));
    assert!(policy.should_compact(0, 1_000));
}

#[test]
fn replacement_history_must_not_be_empty() {
    assert!(CompactionOutput::new(Vec::new()).is_err());
}

#[tokio::test]
async fn scripted_compactor_returns_complete_provider_neutral_history() {
    let expected = vec![Message::System("summary".into())];
    let compactor = ScriptedCompactor::new([CompactionOutput::new(expected.clone()).unwrap()]);

    let output = compactor
        .compact(CompactionRequest::new(
            vec![Message::user_text("long history")],
            crate::CancellationToken::new(),
        ))
        .await
        .unwrap();

    assert_eq!(output.messages(), expected);
}

#[test]
fn compaction_state_tracks_token_and_cost_accounting() {
    use crate::{model::ModelUsage, CompactionState, Revision};

    let mut state = CompactionState::default();
    state.record(4, 1_200, 300, Some(2_500), Revision::from_u64(3));

    assert_eq!(state.completed_compactions(), 1);
    assert_eq!(state.removed_messages(), 4);
    assert_eq!(state.removed_tokens(), 900);
    assert_eq!(state.removed_cost_usd_micros(), 2_500);
    assert_eq!(state.last_previous_tokens(), Some(1_200));
    assert_eq!(state.last_current_tokens(), Some(300));

    let restored = CompactionState::from_parts(1, 4, Some(Revision::from_u64(3)));
    assert_eq!(restored.removed_tokens(), 0);
    assert_eq!(restored.removed_cost_usd_micros(), 0);

    let usage = ModelUsage {
        cost_usd_micros: Some(100),
        ..ModelUsage::default()
    };
    let output =
        CompactionOutput::with_usage(vec![Message::System("summary".into())], usage).unwrap();
    assert_eq!(output.usage().cost_usd_micros, Some(100));
}
