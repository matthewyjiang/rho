pub mod support;

use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use rho_sdk::{
    model::{Message, ModelEvent, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    CompactionFuture, CompactionOutput, CompactionPolicy, CompactionRequest, Compactor, Error, Rho,
    RunEvent, SessionOptions, UserInput,
};

use support::{identity, text_response, tool_call_response, LargeOutputTool, TEST_TIMEOUT};

#[derive(Clone)]
struct BoundedCompactor {
    calls: Arc<AtomicUsize>,
    largest_input: Arc<AtomicUsize>,
}

impl Compactor for BoundedCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(async move {
            if request.cancellation().is_cancelled() {
                return Err(Error::Cancelled);
            }
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.largest_input
                .fetch_max(request.messages().len(), Ordering::AcqRel);
            CompactionOutput::new(vec![Message::System(format!(
                "summary of {} messages",
                request.messages().len()
            ))])
        })
    }
}

#[tokio::test]
async fn long_histories_remain_bounded_across_repeated_automatic_compaction() {
    const RUNS: usize = 256;
    const TRIGGER: usize = 16;
    let turns =
        (0..RUNS).map(|index| ScriptedTurn::completed(text_response(format!("answer {index}"))));
    let provider = ScriptedProvider::new(identity(), turns);
    let calls = Arc::new(AtomicUsize::new(0));
    let largest_input = Arc::new(AtomicUsize::new(0));
    let runtime = Rho::builder()
        .provider(provider.clone())
        .compactor(BoundedCompactor {
            calls: Arc::clone(&calls),
            largest_input: Arc::clone(&largest_input),
        })
        .compaction_policy(CompactionPolicy::after_messages(
            NonZeroUsize::new(TRIGGER).unwrap(),
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    for index in 0..RUNS {
        let outcome = tokio::time::timeout(TEST_TIMEOUT, session.complete(format!("turn {index}")))
            .await
            .expect("long-history run stalled")
            .unwrap();
        assert_eq!(outcome.text(), format!("answer {index}"));
        assert!(session.history().len() <= TRIGGER + 1);
    }

    let completed = calls.load(Ordering::Acquire);
    assert!(completed > 20, "only {completed} compactions ran");
    assert!(largest_input.load(Ordering::Acquire) <= TRIGGER + 1);
    assert_eq!(
        session.snapshot().compaction().completed_compactions(),
        completed as u64
    );
    assert_eq!(provider.recorded_requests().len(), RUNS);
}

#[tokio::test]
async fn large_tool_output_has_one_history_copy_and_one_bounded_semantic_event() {
    const OUTPUT_BYTES: usize = 2 * 1024 * 1024;
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::completed(tool_call_response("large-call", "large")),
            ScriptedTurn::completed(text_response("done")),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .tool(LargeOutputTool {
            bytes: OUTPUT_BYTES,
        })
        .event_capacity(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start(UserInput::text("large output"))
        .await
        .unwrap();
    let mut completed_output_bytes = None;
    let mut tool_finished_events = 0;

    while let Some(event) = tokio::time::timeout(TEST_TIMEOUT, run.next_event())
        .await
        .expect("large output event delivery stalled")
    {
        if let RunEvent::ToolFinished {
            result: rho_sdk::ToolCompletion::Success(output),
            ..
        } = event
        {
            completed_output_bytes = Some(output.content().len());
            tool_finished_events += 1;
        }
    }
    assert_eq!(run.outcome().await.unwrap().text(), "done");
    assert_eq!(completed_output_bytes, Some(OUTPUT_BYTES));
    assert_eq!(tool_finished_events, 1);

    let history = session.history();
    assert_eq!(history.len(), 4);
    let result_bytes = history
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result.content.len()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(result_bytes, [OUTPUT_BYTES]);
    let json = session.snapshot().to_json().unwrap();
    assert!(json.len() >= OUTPUT_BYTES);
    assert!(json.len() < OUTPUT_BYTES + 16 * 1024);
}

#[tokio::test]
async fn malformed_provider_streams_retry_once_then_fail_without_history_growth() {
    const EVENTS_PER_ATTEMPT: usize = 512;
    let malformed_events = (0..EVENTS_PER_ATTEMPT)
        .map(|index| ModelEvent::ToolCallDelta {
            index: index % 8,
            id: (index % 64 == 0).then(|| format!("call-{index}")),
            name: (index % 64 == 0).then(|| "malformed".to_owned()),
            arguments: "{".into(),
        })
        .collect::<Vec<_>>();
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::streaming(
                malformed_events.clone(),
                ModelResponse::Assistant(Vec::new()),
            ),
            ScriptedTurn::streaming(malformed_events, ModelResponse::Assistant(Vec::new())),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider.clone())
        .event_capacity(NonZeroUsize::new(8).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("malformed")).await.unwrap();
    let mut fragment_events = 0;
    let mut retry_events = 0;
    let mut terminal_failures = 0;

    while let Some(event) = tokio::time::timeout(TEST_TIMEOUT, run.next_event())
        .await
        .expect("malformed stream stalled")
    {
        match event {
            RunEvent::ToolCallUpdated { .. } => fragment_events += 1,
            RunEvent::ProviderActivity { kind, .. } if kind == "invalid_response_retry" => {
                retry_events += 1;
            }
            RunEvent::Failed { .. } => terminal_failures += 1,
            _ => {}
        }
    }
    let error = run.outcome().await.unwrap_err();
    assert!(matches!(error, Error::Provider(_)));
    assert_eq!(fragment_events, EVENTS_PER_ATTEMPT * 2);
    assert_eq!(retry_events, 1);
    assert_eq!(terminal_failures, 1);
    assert_eq!(provider.recorded_requests().len(), 2);
    assert!(session.history().is_empty());
}

#[test]
fn representative_long_snapshot_serialization_is_linear_in_history_size() {
    const MESSAGES: usize = 10_000;
    let history = (0..MESSAGES)
        .map(|index| Message::user_text(format!("message-{index:05}-{}", "x".repeat(32))))
        .collect::<Vec<_>>();
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(identity(), []))
        .build()
        .unwrap();
    let tokio = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let session = tokio
        .block_on(runtime.session(SessionOptions::default().history(history)))
        .unwrap();

    let json = session.snapshot().to_json().unwrap();
    assert!(json.len() > MESSAGES * 32);
    assert!(json.len() < MESSAGES * 128);
    assert_eq!(session.history().len(), MESSAGES);
}
