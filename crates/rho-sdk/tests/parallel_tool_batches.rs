mod support;

use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use rho_sdk::{
    model::{ContentBlock, Message, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{
        PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture,
        ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
        ToolResource, ToolResourceAccess,
    },
    Rho, Run, RunEvent, Session, SessionOptions, ToolCompletion, UserInput,
};
use serde_json::json;
use tokio::sync::{mpsc, Semaphore};

use support::{identity, text_response, TEST_TIMEOUT};

#[derive(Default)]
struct BatchProbe {
    started_tx: Mutex<Option<mpsc::UnboundedSender<String>>>,
    gates: Mutex<BTreeMap<String, Arc<Semaphore>>>,
    active: AtomicUsize,
    peak: AtomicUsize,
}

impl BatchProbe {
    fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<String>) {
        let (started_tx, started_rx) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                started_tx: Mutex::new(Some(started_tx)),
                ..Self::default()
            }),
            started_rx,
        )
    }

    fn gate(&self, key: &str) -> Arc<Semaphore> {
        let mut gates = self.gates.lock().unwrap();
        Arc::clone(
            gates
                .entry(key.to_owned())
                .or_insert_with(|| Arc::new(Semaphore::new(0))),
        )
    }

    fn release(&self, key: &str) {
        self.gate(key).add_permits(1);
    }

    fn mark_started(&self, key: &str) {
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.peak.fetch_max(active, Ordering::AcqRel);
        self.started_tx
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(key.to_owned())
            .unwrap();
    }

    async fn next_started(started: &mut mpsc::UnboundedReceiver<String>) -> String {
        tokio::time::timeout(TEST_TIMEOUT, started.recv())
            .await
            .expect("tool did not start")
            .expect("start probe closed")
    }
}

#[derive(Clone, Copy)]
enum Scheduling {
    Shared,
    Write,
    Empty,
    DefaultExclusive,
}

#[derive(Clone)]
struct ControlledTool {
    name: &'static str,
    scheduling: Scheduling,
    probe: Arc<BatchProbe>,
    failures: Arc<BTreeSet<String>>,
}

impl ControlledTool {
    fn new(name: &'static str, scheduling: Scheduling, probe: Arc<BatchProbe>) -> Self {
        Self {
            name,
            scheduling,
            probe,
            failures: Arc::default(),
        }
    }

    fn failing(mut self, keys: impl IntoIterator<Item = &'static str>) -> Self {
        self.failures = Arc::new(keys.into_iter().map(str::to_owned).collect());
        self
    }

    fn execute<'a>(&'a self, key: String, gate: Arc<Semaphore>) -> ToolFuture<'a> {
        Box::pin(async move {
            self.probe.mark_started(&key);
            let permit = gate.acquire().await.unwrap();
            permit.forget();
            self.probe.active.fetch_sub(1, Ordering::AcqRel);
            if self.failures.contains(&key) {
                Err(ToolError::new(
                    ToolErrorKind::Execution,
                    format!("failed {key}"),
                ))
            } else {
                Ok(ToolOutput::text(format!("result-{key}")))
            }
        })
    }
}

impl Tool for ControlledTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "deterministic parallel batch probe".into(),
            input_schema: json!({"type":"object","required":["key","resource"]}),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        let key = invocation.arguments()["key"].as_str().unwrap().to_owned();
        self.execute(key.clone(), self.probe.gate(&key))
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        if matches!(self.scheduling, Scheduling::DefaultExclusive) {
            return Box::pin(async move {
                Ok(PreparedToolInvocation::exclusive(
                    ToolMetadata::new(),
                    move |context| self.call(invocation, context),
                ))
            });
        }
        let key = invocation.arguments()["key"].as_str().unwrap().to_owned();
        let resource = invocation.arguments()["resource"]
            .as_str()
            .unwrap()
            .to_owned();
        let gate = self.probe.gate(&key);
        let accesses = match self.scheduling {
            Scheduling::Shared => vec![ToolResourceAccess::shared(ToolResource::opaque(
                "batch-test",
                resource,
            ))],
            Scheduling::Write => vec![ToolResourceAccess::exclusive(ToolResource::opaque(
                "batch-test",
                resource,
            ))],
            Scheduling::Empty => Vec::new(),
            Scheduling::DefaultExclusive => unreachable!(),
        };
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                accesses,
                [],
                ToolMetadata::new(),
                move |_context| self.execute(key, gate),
            ))
        })
    }
}

#[derive(Clone)]
struct LegacyTool {
    probe: Arc<BatchProbe>,
}

impl Tool for LegacyTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "legacy".into(),
            description: "tool using the default exclusive preparation".into(),
            input_schema: json!({"type":"object"}),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        let key = invocation.arguments()["key"].as_str().unwrap().to_owned();
        let gate = self.probe.gate(&key);
        Box::pin(async move {
            self.probe.mark_started(&key);
            gate.acquire().await.unwrap().forget();
            self.probe.active.fetch_sub(1, Ordering::AcqRel);
            Ok(ToolOutput::text(format!("result-{key}")))
        })
    }
}

fn call(id: &str, name: &str, key: &str, resource: &str) -> ContentBlock {
    ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: json!({"key": key, "resource": resource}),
    })
}

fn provider(calls: Vec<ContentBlock>) -> ScriptedProvider {
    ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(calls)),
            ScriptedTurn::completed(text_response("done")),
        ],
    )
}

async fn start_session(
    provider: ScriptedProvider,
    tools: impl IntoIterator<Item = ControlledTool>,
    limit: usize,
) -> (Session, Run) {
    let mut builder = Rho::builder()
        .provider(provider)
        .max_parallel_tools(NonZeroUsize::new(limit).unwrap());
    for tool in tools {
        builder = builder.tool(tool);
    }
    let session = builder
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    let run = session.start(UserInput::text("batch")).await.unwrap();
    (session, run)
}

async fn finish(mut run: Run) {
    while run.next_event().await.is_some() {}
    assert_eq!(run.outcome().await.unwrap().text(), "done");
}

#[tokio::test]
async fn independent_resource_aware_calls_overlap_and_enforce_the_active_worker_bound() {
    let (probe, mut started) = BatchProbe::channel();
    let calls = (0..4)
        .map(|index| {
            call(
                &format!("call-{index}"),
                "read",
                &format!("k{index}"),
                &format!("r{index}"),
            )
        })
        .collect();
    let (_, run) = start_session(
        provider(calls),
        [ControlledTool::new(
            "read",
            Scheduling::Shared,
            Arc::clone(&probe),
        )],
        2,
    )
    .await;

    let first = BatchProbe::next_started(&mut started).await;
    let second = BatchProbe::next_started(&mut started).await;
    assert_ne!(first, second);
    assert_eq!(probe.peak.load(Ordering::Acquire), 2);
    probe.release(&first);
    let third = BatchProbe::next_started(&mut started).await;
    probe.release(&second);
    let fourth = BatchProbe::next_started(&mut started).await;
    assert_eq!(probe.peak.load(Ordering::Acquire), 2);
    probe.release(&third);
    probe.release(&fourth);
    finish(run).await;
}

#[tokio::test]
async fn same_resource_writes_serialize_in_model_order() {
    let (probe, mut started) = BatchProbe::channel();
    let (_, run) = start_session(
        provider(vec![
            call("first", "write", "first", "same"),
            call("second", "write", "second", "same"),
        ]),
        [ControlledTool::new(
            "write",
            Scheduling::Write,
            Arc::clone(&probe),
        )],
        2,
    )
    .await;

    assert_eq!(BatchProbe::next_started(&mut started).await, "first");
    probe.release("first");
    assert_eq!(BatchProbe::next_started(&mut started).await, "second");
    probe.release("second");
    finish(run).await;
    assert_eq!(probe.peak.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn exclusive_call_is_a_barrier_that_later_calls_cannot_overtake() {
    let (probe, mut started) = BatchProbe::channel();
    let (_, run) = start_session(
        provider(vec![
            call("before", "aware", "before", "a"),
            call("barrier", "legacy", "barrier", "b"),
            call("after", "aware", "after", "c"),
        ]),
        [
            ControlledTool::new("aware", Scheduling::Empty, Arc::clone(&probe)),
            ControlledTool::new("legacy", Scheduling::DefaultExclusive, Arc::clone(&probe)),
        ],
        3,
    )
    .await;

    assert_eq!(BatchProbe::next_started(&mut started).await, "before");
    probe.release("before");
    assert_eq!(BatchProbe::next_started(&mut started).await, "barrier");
    probe.release("barrier");
    assert_eq!(BatchProbe::next_started(&mut started).await, "after");
    probe.release("after");
    finish(run).await;
    assert_eq!(probe.peak.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn out_of_order_completion_keeps_provider_and_snapshot_history_in_model_order() {
    let (probe, mut started) = BatchProbe::channel();
    let scripted = provider(vec![
        call("first", "read", "first", "a"),
        call("second", "read", "second", "b"),
    ]);
    let recorded = scripted.clone();
    let (session, run) = start_session(
        scripted,
        [ControlledTool::new(
            "read",
            Scheduling::Shared,
            Arc::clone(&probe),
        )],
        2,
    )
    .await;

    let mut keys = BTreeSet::new();
    keys.insert(BatchProbe::next_started(&mut started).await);
    keys.insert(BatchProbe::next_started(&mut started).await);
    assert_eq!(
        keys,
        BTreeSet::from(["first".to_owned(), "second".to_owned()])
    );
    probe.release("second");
    while probe.active.load(Ordering::Acquire) != 1 {
        tokio::task::yield_now().await;
    }
    probe.release("first");
    finish(run).await;

    let requests = recorded.recorded_requests();
    let provider_results = requests[1]
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result.content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(provider_results, ["result-first", "result-second"]);
    let restored =
        rho_sdk::SessionSnapshot::from_json(&session.snapshot().to_json().unwrap()).unwrap();
    let persisted_results = restored
        .history()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result.content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(persisted_results, ["result-first", "result-second"]);
}

#[tokio::test]
async fn unavailable_call_does_not_block_independent_available_work() {
    let (probe, mut started) = BatchProbe::channel();
    let (_, mut run) = start_session(
        provider(vec![
            call("missing", "not_registered", "missing", "x"),
            call("available", "read", "available", "y"),
        ]),
        [ControlledTool::new(
            "read",
            Scheduling::Empty,
            Arc::clone(&probe),
        )],
        2,
    )
    .await;

    assert_eq!(BatchProbe::next_started(&mut started).await, "available");
    probe.release("available");
    let mut unavailable = false;
    while let Some(event) = run.next_event().await {
        unavailable |= matches!(
            event,
            RunEvent::ToolFinished {
                result: ToolCompletion::Unavailable,
                ..
            }
        );
    }
    assert!(unavailable);
    assert_eq!(run.outcome().await.unwrap().text(), "done");
}

#[tokio::test]
async fn default_tool_implementation_remains_exclusive() {
    let (probe, mut started) = BatchProbe::channel();
    let runtime = Rho::builder()
        .provider(provider(vec![
            call("one", "legacy", "one", "a"),
            call("two", "legacy", "two", "b"),
        ]))
        .tool(LegacyTool {
            probe: Arc::clone(&probe),
        })
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let run = session
        .start(UserInput::text("legacy batch"))
        .await
        .unwrap();

    assert_eq!(BatchProbe::next_started(&mut started).await, "one");
    probe.release("one");
    assert_eq!(BatchProbe::next_started(&mut started).await, "two");
    probe.release("two");
    finish(run).await;
    assert_eq!(probe.peak.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn one_tool_failure_does_not_cancel_an_unrelated_sibling() {
    let (probe, mut started) = BatchProbe::channel();
    let tool = ControlledTool::new("work", Scheduling::Empty, Arc::clone(&probe)).failing(["bad"]);
    let (_, mut run) = start_session(
        provider(vec![
            call("bad", "work", "bad", "a"),
            call("good", "work", "good", "b"),
        ]),
        [tool],
        2,
    )
    .await;

    let first = BatchProbe::next_started(&mut started).await;
    let second = BatchProbe::next_started(&mut started).await;
    assert_eq!(
        BTreeSet::from([first, second]),
        BTreeSet::from(["bad".into(), "good".into()])
    );
    probe.release("bad");
    while probe.active.load(Ordering::Acquire) != 1 {
        tokio::task::yield_now().await;
    }
    probe.release("good");
    let mut completions = BTreeMap::<String, bool>::new();
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolFinished { call_id, result } = event {
            completions.insert(
                call_id.to_string(),
                matches!(result, ToolCompletion::Success(_)),
            );
        }
    }
    assert_eq!(run.outcome().await.unwrap().text(), "done");
    assert_eq!(completions.get("bad"), Some(&false));
    assert_eq!(completions.get("good"), Some(&true));
}

#[tokio::test]
async fn duplicate_model_tool_call_ids_are_rejected_before_any_worker_starts() {
    let (probe, mut started) = BatchProbe::channel();
    let (_, mut run) = start_session(
        provider(vec![
            call("duplicate", "read", "one", "a"),
            call("duplicate", "read", "two", "b"),
        ]),
        [ControlledTool::new("read", Scheduling::Empty, probe)],
        2,
    )
    .await;

    while run.next_event().await.is_some() {}
    assert_eq!(run.outcome().await.unwrap().text(), "done");
    assert!(started.try_recv().is_err());
}
