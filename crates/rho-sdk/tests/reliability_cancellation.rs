pub mod support;

use std::{num::NonZeroUsize, sync::atomic::Ordering};

use rho_sdk::{
    model::{Message, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolFuture, ToolInvocation, ToolProgress},
    CompactionFuture, CompactionPolicy, CompactionRequest, Compactor, Error, Rho, Run,
    SessionOptions, UserInput,
};
use serde_json::json;

use support::{
    identity, tool_call_response, DropGuard, FloodProvider, PendingProvider, PendingTool, Probe,
    TEST_TIMEOUT,
};

async fn cancelled_outcome(run: &mut Run) {
    let result = tokio::time::timeout(TEST_TIMEOUT, run.outcome())
        .await
        .expect("cancellation did not finish within the reliability deadline");
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
}

#[tokio::test]
async fn provider_await_is_cancelled_and_dropped_without_an_orphan_task() {
    let probe = Probe::default();
    let session = support::session_with(PendingProvider {
        probe: probe.clone(),
    })
    .await;
    let mut run = session.start(UserInput::text("wait")).await.unwrap();
    probe.wait_started().await;

    run.cancel();
    cancelled_outcome(&mut run).await;
    probe.wait_dropped().await;
    assert!(!session.is_running());
}

#[tokio::test]
async fn full_provider_and_host_event_channels_cancel_with_bounded_production() {
    const REQUESTED_EVENTS: usize = 100_000;
    let probe = Probe::default();
    let runtime = Rho::builder()
        .provider(FloodProvider {
            events: REQUESTED_EVENTS,
            probe: probe.clone(),
        })
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("flood")).await.unwrap();
    while !probe.started.load(Ordering::Acquire) {
        tokio::time::timeout(TEST_TIMEOUT, run.next_event())
            .await
            .expect("event channel stalled before provider start");
    }

    for _ in 0..128 {
        tokio::task::yield_now().await;
    }
    let produced_at_backpressure = probe.produced.load(Ordering::Acquire);
    assert!(
        produced_at_backpressure < 32,
        "provider produced {produced_at_backpressure} events despite bounded channels"
    );

    run.cancel();
    cancelled_outcome(&mut run).await;
    probe.wait_dropped().await;
    assert!(probe.produced.load(Ordering::Acquire) < REQUESTED_EVENTS);
}

async fn run_pending_tool(request_host_input: bool) {
    let probe = Probe::default();
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(tool_call_response(
            "pending-call",
            "pending",
        ))],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .tool(PendingTool {
            probe: probe.clone(),
            request_host_input,
        })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("call tool")).await.unwrap();
    probe.wait_started().await;

    if request_host_input {
        loop {
            if matches!(
                run.next_event().await,
                Some(rho_sdk::RunEvent::HostInputRequested { .. })
            ) {
                break;
            }
        }
    }
    run.cancel();
    cancelled_outcome(&mut run).await;
    probe.wait_dropped().await;
    assert!(!session.is_running());
}

#[tokio::test]
async fn tool_await_is_cancelled_and_dropped_without_an_orphan_task() {
    run_pending_tool(false).await;
}

#[tokio::test]
async fn host_input_response_await_is_cancelled_and_dropped_without_a_responder() {
    run_pending_tool(true).await;
}

#[derive(Clone)]
struct PendingCompactor {
    probe: Probe,
}

impl Compactor for PendingCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(async move {
            let _guard = DropGuard::new(&self.probe);
            self.probe.started.store(true, Ordering::Release);
            request.cancellation().cancelled().await;
            Err(Error::Cancelled)
        })
    }
}

#[tokio::test]
async fn automatic_compaction_await_is_cancelled_without_committing_partial_history() {
    let probe = Probe::default();
    let runtime = Rho::builder()
        .provider(PendingProvider {
            probe: Probe::default(),
        })
        .compactor(PendingCompactor {
            probe: probe.clone(),
        })
        .compaction_policy(CompactionPolicy::after_messages(
            NonZeroUsize::new(1).unwrap(),
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("compact")).await.unwrap();
    probe.wait_started().await;

    run.cancel();
    cancelled_outcome(&mut run).await;
    probe.wait_dropped().await;
    assert!(matches!(session.history().as_slice(), [Message::User(_)]));
    assert_eq!(session.snapshot().compaction().completed_compactions(), 0);
}

#[tokio::test]
async fn manual_compaction_await_observes_runtime_shutdown_and_drops_transport() {
    let probe = Probe::default();
    let runtime = Rho::builder()
        .provider(PendingProvider {
            probe: Probe::default(),
        })
        .compactor(PendingCompactor {
            probe: probe.clone(),
        })
        .build()
        .unwrap();
    let session = runtime
        .session(SessionOptions::default().history(vec![Message::user_text("history")]))
        .await
        .unwrap();
    let compacting = session.clone();
    let task = tokio::spawn(async move { compacting.compact().await });
    probe.wait_started().await;

    assert_eq!(runtime.shutdown().cancelled_runs(), 1);
    let result = tokio::time::timeout(TEST_TIMEOUT, task)
        .await
        .expect("manual compaction ignored shutdown")
        .unwrap();
    assert!(matches!(result, Err(Error::Cancelled)));
    probe.wait_dropped().await;
    assert_eq!(session.snapshot().compaction().completed_compactions(), 0);
    assert!(!session.is_running());
}

#[derive(Clone)]
struct ProgressFloodTool {
    probe: Probe,
}

impl Tool for ProgressFloodTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "progress_flood".into(),
            description: "fill bounded progress channels".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let _guard = DropGuard::new(&self.probe);
            self.probe.started.store(true, Ordering::Release);
            loop {
                if !context.progress().send(ToolProgress::message("tick")).await {
                    return Err(rho_sdk::tool::ToolError::cancelled());
                }
                self.probe.produced.fetch_add(1, Ordering::AcqRel);
            }
        })
    }
}

#[tokio::test]
async fn full_tool_progress_channel_drops_the_tool_future_on_cancellation() {
    let probe = Probe::default();
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(tool_call_response(
            "progress-call",
            "progress_flood",
        ))],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .tool(ProgressFloodTool {
            probe: probe.clone(),
        })
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("progress")).await.unwrap();

    while !probe.started.load(Ordering::Acquire) {
        tokio::time::timeout(TEST_TIMEOUT, run.next_event())
            .await
            .unwrap();
    }
    for _ in 0..128 {
        tokio::task::yield_now().await;
    }
    assert!(probe.produced.load(Ordering::Acquire) <= 18);

    run.cancel();
    cancelled_outcome(&mut run).await;
    probe.wait_dropped().await;
}

#[tokio::test]
async fn dropping_event_consumer_aborts_and_drops_provider_work() {
    let probe = Probe::default();
    let session = support::session_with(PendingProvider {
        probe: probe.clone(),
    })
    .await;
    let run = session.start(UserInput::text("drop")).await.unwrap();
    probe.wait_started().await;

    drop(run);
    probe.wait_dropped().await;
    tokio::time::timeout(TEST_TIMEOUT, async {
        while session.is_running() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropped run left the session worker registered");
}

#[test]
fn cancellation_harness_types_remain_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PendingProvider>();
    assert_send_sync::<PendingCompactor>();
    assert_send_sync::<ProgressFloodTool>();
}
