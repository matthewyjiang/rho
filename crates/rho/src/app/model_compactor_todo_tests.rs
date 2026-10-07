use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{
        context::{estimate_context_tokens, estimate_messages_tokens},
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelResponse, ModelUsage,
    },
    provider::{ScriptedProvider, ScriptedTurn},
    CancellationToken, CompactionOutput, CompactionRequest, Compactor, Error, RequestContext,
    SessionId,
};
use serde_json::json;

use super::tests::{compactor, RecordingUsage};
use crate::tools::todo::{TodoList, TodoState};

// Covers: mandatory live state must reserve context before tier selection, reject
// an impossible actual budget, and fall back from a native output that crowds it
// out. The text-summary tier must retain the latest genuine human instruction.
// Owner: host compaction budgeting and tail selection.
#[tokio::test]
async fn todo_context_is_budgeted_before_native_and_summary_tiers() {
    let state = TodoState::default();
    state.replace(Some(
        TodoList::parse(json!({"todos": [{
            "content": "exact task state", "status": "pending",
        }]}))
        .unwrap(),
    ));
    let context = state.messages(&SessionId::new());
    let retained = estimate_messages_tokens(&context);
    let minimum = estimate_context_tokens(&context, &[]);
    let latest = Message::user_text("keep this actual latest instruction");
    let history = vec![
        Message::user_text("initial request"),
        Message::assistant_text("old content ".repeat(10_000)),
        latest.clone(),
    ];
    let oversized = vec![Message::assistant_text("native content ".repeat(1_000))];
    // Both capacities derive from actual estimator footprints, not product caps.
    let target = estimate_context_tokens(&oversized, &[]) + retained - 1;
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
    let mut compactor = compactor(
        provider.clone(),
        RecordingUsage::default(),
        Some(target * 5),
    );
    compactor.todo = Some(state.clone());
    let output = compactor
        .compact(CompactionRequest::new(
            history.clone(),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
    assert_eq!(provider.recorded_requests().len(), 2);
    assert_eq!(*output.usage(), native_usage.saturating_add(&summary_usage));
    assert!(output.messages().iter().any(|message| message == &latest));
    assert!(estimate_context_tokens(output.messages(), &[]) + retained <= target);

    let provider = ScriptedProvider::new(ModelIdentity::new("test", "test", "test"), []);
    let limit = minimum - 1;
    let mut compactor =
        super::tests::compactor(provider.clone(), RecordingUsage::default(), Some(limit * 5));
    compactor.todo = Some(state);
    let error = compactor
        .compact(CompactionRequest::new(history, CancellationToken::new()))
        .await
        .unwrap_err();
    let Error::InvalidConfiguration { message } = error else {
        panic!("expected mandatory context budget rejection, got {error:?}");
    };
    assert_eq!(message, format!("mandatory task context exceeds compaction context budget: limit {limit} estimated tokens, asked {minimum}"));
    assert_eq!(provider.recorded_requests().len(), 0);
}
