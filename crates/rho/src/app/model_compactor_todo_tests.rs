use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{
        context::{estimate_context_tokens, estimate_messages_tokens},
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelResponse, ModelUsage,
    },
    provider::{ScriptedProvider, ScriptedTurn},
    CancellationToken, CompactionOutput, CompactionRequest, Compactor,
};
use serde_json::json;

use super::tests::{compactor, RecordingUsage};
use crate::tools::todo::{is_restatement, TodoList, TodoState};

fn todo_state(content: &str) -> TodoState {
    let state = TodoState::default();
    state.replace(Some(
        TodoList::parse(json!({"todos": [{"content": content, "status": "pending"}]})).unwrap(),
    ));
    state
}

fn summary_provider() -> ScriptedProvider {
    ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("short summary".into()),
        ]))],
    )
}

// Covers: the restatement is appended when the replacement has room for it,
// and skipped (list kept in metadata only) when it would overflow the window.
// Owner: host compaction budgeting for the checklist restatement.
#[tokio::test]
async fn restatement_is_appended_only_when_it_fits_the_window() {
    let state = todo_state("small task");
    let retained = estimate_messages_tokens(&[state.checkpoint().restatement().unwrap()]);
    let history = vec![
        Message::System("fixed prompt ".repeat(1_000)),
        Message::user_text("brief request"),
        Message::assistant_text("old answer ".repeat(20)),
        Message::user_text("recent instruction"),
    ];
    let roomy = estimate_context_tokens(&history, &[]) * 2;
    let mut roomy_compactor = compactor(summary_provider(), RecordingUsage::default(), Some(roomy));
    roomy_compactor.todo = Some(state.clone());
    let output = roomy_compactor
        .compact(CompactionRequest::new(
            history.clone(),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
    let (last, base) = output.messages().split_last().unwrap();
    assert!(is_restatement(last));
    assert!(estimate_context_tokens(output.messages(), &[]) <= roomy);

    // One token short of the same replacement plus the restatement.
    let tight = estimate_context_tokens(base, &[]) + retained - 1;
    let mut compactor = compactor(summary_provider(), RecordingUsage::default(), Some(tight));
    compactor.todo = Some(state);
    let output = compactor
        .compact(CompactionRequest::new(history, CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(output.messages(), base);
}

// Covers: a native replacement that leaves no room for the restatement falls
// back to the text summary, which keeps the latest genuine human instruction.
// Owner: host compaction tier selection.
#[tokio::test]
async fn native_output_that_crowds_out_the_restatement_falls_back_to_summary() {
    let state = todo_state("exact task state");
    let retained = estimate_messages_tokens(&[state.checkpoint().restatement().unwrap()]);
    let latest = Message::user_text("keep this actual latest instruction");
    let history = vec![
        Message::user_text("initial request"),
        Message::assistant_text("old content ".repeat(10_000)),
        latest.clone(),
    ];
    let oversized = vec![Message::assistant_text("native content ".repeat(1_000))];
    let capacity = estimate_context_tokens(&oversized, &[]) + retained - 1;
    let native_usage = ModelUsage {
        input_tokens: Some(200),
        output_tokens: Some(80),
        cost_usd_micros: Some(1_000),
        ..ModelUsage::default()
    };
    let summary_usage = ModelUsage {
        input_tokens: Some(100),
        output_tokens: Some(20),
        cost_usd_micros: Some(500),
        ..ModelUsage::default()
    };
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [ScriptedTurn::streaming(
            vec![ModelEvent::Usage(summary_usage.clone())],
            ModelResponse::Assistant(vec![ContentBlock::Text("concise summary".into())]),
        )],
    )
    .with_native_compactions([Ok(CompactionOutput::with_usage(
        oversized,
        native_usage.clone(),
    )
    .unwrap())]);
    let mut compactor = compactor(provider.clone(), RecordingUsage::default(), Some(capacity));
    compactor.todo = Some(state);
    let output = compactor
        .compact(CompactionRequest::new(history, CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(provider.recorded_requests().len(), 2);
    assert_eq!(*output.usage(), native_usage.saturating_add(&summary_usage));
    assert!(output.messages().contains(&latest));
    assert!(output.messages().last().is_some_and(is_restatement));
    assert!(estimate_context_tokens(output.messages(), &[]) <= capacity);
}
