use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{
        context::{estimate_context_tokens, estimate_messages_tokens},
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelResponse, ModelUsage,
    },
    provider::{ScriptedProvider, ScriptedTurn},
    CancellationToken, CompactionOutput, CompactionRequest, Compactor, RequestContext, SessionId,
};
use serde_json::json;

use super::tests::{compactor, RecordingUsage};
use crate::tools::todo::{TodoList, TodoState};

// Covers: mandatory prompt/recent history may exceed the soft retention goal
// without exceeding the model window, including after clearing the checklist.
// Owner: host compaction budgeting.
#[tokio::test]
async fn todo_does_not_make_the_retention_target_a_capacity_limit() {
    let fixed = Message::System("fixed prompt ".repeat(1_000));
    for (history, expected_requests) in [
        (vec![fixed.clone(), Message::user_text("brief request")], 0),
        (
            vec![
                fixed.clone(),
                Message::user_text("brief request"),
                Message::assistant_text("old answer ".repeat(20)),
                Message::user_text("recent instruction"),
            ],
            1,
        ),
    ] {
        for todos in [
            json!([]),
            json!([{"content": "small task", "status": "pending"}]),
        ] {
            let state = TodoState::default();
            state.replace(Some(TodoList::parse(json!({"todos": todos})).unwrap()));
            let retained = estimate_messages_tokens(&state.messages(&SessionId::new()));
            let current = estimate_context_tokens(&history, &[]) + retained;
            let capacity = current * 2;
            let provider = ScriptedProvider::new(
                ModelIdentity::new("test", "test", "test"),
                [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                    ContentBlock::Text("short summary".into()),
                ]))],
            );
            let mut compactor =
                compactor(provider.clone(), RecordingUsage::default(), Some(capacity));
            compactor.todo = Some(state);
            let output = compactor
                .compact(CompactionRequest::new(
                    history.clone(),
                    CancellationToken::new(),
                ))
                .await
                .unwrap();
            assert_eq!(output.messages().first(), Some(&fixed));
            assert_eq!(output.messages().last(), history.last());
            let replacement = estimate_context_tokens(output.messages(), &[]) + retained;
            assert!(replacement > current / 2);
            assert!(replacement <= capacity);
            assert_eq!(provider.recorded_requests().len(), expected_requests);
        }
    }
}

// Covers: reserve live state before tier selection, suspend impossible projection,
// and fall back from native output that crowds it out. The text-summary tier must
// retain the latest genuine human instruction.
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
    let exact = state.list();
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
    let capacity = estimate_context_tokens(&oversized, &[]) + retained - 1;
    let target = capacity / 5;
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
        super::tests::compactor(provider.clone(), RecordingUsage::default(), Some(limit));
    compactor.todo = Some(state.clone());
    let irreducible = vec![Message::user_text("kept")];
    let output = compactor
        .compact(CompactionRequest::new(
            irreducible.clone(),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
    assert_eq!(output.messages(), irreducible);
    assert_eq!(state.list(), exact);
    let projection = state.messages(&SessionId::new());
    assert_ne!(projection, context);
    assert!(
        estimate_context_tokens(output.messages(), &[]) + estimate_messages_tokens(&projection)
            <= limit
    );
    assert_eq!(provider.recorded_requests().len(), 0);
}
