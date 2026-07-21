mod support;

use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use rho_sdk::{
    approval_channel,
    model::{ContentBlock, Message, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{
        PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture,
        ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
        ToolProgress, ToolResource, ToolResourceAccess, ToolSecurity,
    },
    ApprovalDecision, CapabilityKind, CapabilityRequest, CapabilitySource, Error, HostChoice,
    HostInputRequest, HostInputResponse, NetworkTarget, Rho, RunEvent, ScopedWorkspacePolicy,
    SelectionMode, SessionOptions, UserInput,
};
use serde_json::json;
use tokio::sync::{mpsc, Semaphore};

use support::{identity, text_response, TEST_TIMEOUT};

struct InteractionProbe {
    started: mpsc::UnboundedSender<String>,
    gates: Mutex<BTreeMap<String, Arc<Semaphore>>>,
}

impl InteractionProbe {
    fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<String>) {
        let (started, receiver) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                started,
                gates: Mutex::default(),
            }),
            receiver,
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
}

async fn next_started(receiver: &mut mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(TEST_TIMEOUT, receiver.recv())
        .await
        .expect("worker did not start")
        .expect("start channel closed")
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

#[derive(Clone)]
struct CancellableTool {
    probe: Arc<InteractionProbe>,
}

impl Tool for CancellableTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "work".into(),
            description: "cancellable deterministic work".into(),
            input_schema: json!({"type":"object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        unreachable!("resource-aware preparation is always used")
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let key = invocation.arguments()["key"].as_str().unwrap().to_owned();
        let resource = invocation.arguments()["resource"]
            .as_str()
            .unwrap()
            .to_owned();
        let gate = self.probe.gate(&key);
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                [ToolResourceAccess::exclusive(ToolResource::opaque(
                    "interaction-test",
                    resource,
                ))],
                [],
                ToolMetadata::new(),
                move |context| {
                    Box::pin(async move {
                        self.probe.started.send(key.clone()).unwrap();
                        tokio::select! {
                            permit = gate.acquire() => permit.unwrap().forget(),
                            () = context.cancellation().cancelled() => {
                                return Err(ToolError::cancelled());
                            }
                        }
                        Ok(ToolOutput::text(format!("result-{key}")))
                    })
                },
            ))
        })
    }
}

fn question_request(title: &str) -> HostInputRequest {
    let question = rho_sdk::HostQuestion::new(
        "answer",
        "continue?",
        vec![HostChoice::new("yes", "yes")],
        SelectionMode::One,
    )
    .unwrap();
    HostInputRequest::questionnaire(title, vec![question]).unwrap()
}

#[derive(Clone)]
struct HostInputTool {
    shared_request: Option<HostInputRequest>,
    exclusive_resource: bool,
}

impl Tool for HostInputTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ask".into(),
            description: "parallel host input probe".into(),
            input_schema: json!({"type":"object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        unreachable!("resource-aware preparation is always used")
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let key = invocation.arguments()["key"].as_str().unwrap().to_owned();
        let request = self
            .shared_request
            .clone()
            .unwrap_or_else(|| question_request(&key));
        let access = if self.exclusive_resource {
            ToolResourceAccess::exclusive(ToolResource::opaque("host-input-test", key.clone()))
        } else {
            ToolResourceAccess::shared(ToolResource::opaque("host-input-test", key.clone()))
        };
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                [access],
                [],
                ToolMetadata::new(),
                move |context| {
                    Box::pin(async move {
                        let response =
                            context.request_host_input(request).await.map_err(|error| {
                                ToolError::new(ToolErrorKind::Execution, error.to_string())
                            })?;
                        Ok(ToolOutput::text(response.answers()["answer"][0].clone()))
                    })
                },
            ))
        })
    }
}

#[derive(Clone)]
struct PendingPreparationTool {
    started: mpsc::UnboundedSender<()>,
}

impl Tool for PendingPreparationTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "pending_prepare".into(),
            description: "preparation cancellation probe".into(),
            input_schema: json!({"type":"object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        unreachable!("preparation never completes")
    }

    fn prepare<'a>(
        &'a self,
        _invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            self.started.send(()).unwrap();
            std::future::pending().await
        })
    }
}

#[derive(Clone)]
struct FinalProgressTool;

impl Tool for FinalProgressTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "final_progress".into(),
            description: "final progress ordering probe".into(),
            input_schema: json!({"type":"object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            assert!(
                context
                    .progress()
                    .send(ToolProgress::message("final update"))
                    .await
            );
            Ok(ToolOutput::text("complete"))
        })
    }
}

#[derive(Clone)]
struct BackpressureInteractionTool {
    request: HostInputRequest,
    progress_gate: Arc<Semaphore>,
    progress_sent: mpsc::UnboundedSender<()>,
    finish_gate: Arc<Semaphore>,
}

impl Tool for BackpressureInteractionTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "interact".into(),
            description: "command and progress backpressure probe".into(),
            input_schema: json!({"type":"object"}),
        }
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let key = invocation.arguments()["key"].as_str().unwrap().to_owned();
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                [ToolResourceAccess::shared(ToolResource::opaque(
                    "backpressure-test",
                    key,
                ))],
                [],
                ToolMetadata::new(),
                move |context| {
                    Box::pin(async move {
                        let asks_for_input =
                            invocation.arguments()["key"].as_str().unwrap() == "ask";
                        if asks_for_input {
                            let response = context
                                .request_host_input(self.request.clone())
                                .await
                                .map_err(|error| {
                                ToolError::new(ToolErrorKind::Execution, error.to_string())
                            })?;
                            return Ok(ToolOutput::text(response.answers()["answer"][0].clone()));
                        }

                        loop {
                            tokio::select! {
                                biased;
                                permit = self.finish_gate.acquire() => {
                                    permit.unwrap().forget();
                                    return Ok(ToolOutput::text("progress complete"));
                                }
                                permit = self.progress_gate.acquire() => {
                                    permit.unwrap().forget();
                                    if !context.progress().send(ToolProgress::message("tick")).await {
                                        return Err(ToolError::cancelled());
                                    }
                                    self.progress_sent.send(()).unwrap();
                                }
                                () = context.cancellation().cancelled() => {
                                    return Err(ToolError::cancelled());
                                }
                            }
                        }
                    })
                },
            ))
        })
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        unreachable!("resource-aware preparation is always used")
    }
}

#[derive(Clone)]
struct ApprovalTool {
    probe: Arc<InteractionProbe>,
}

impl Tool for ApprovalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "approved".into(),
            description: "approval scheduling probe".into(),
            input_schema: json!({"type":"object"}),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Network])
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        unreachable!("resource-aware preparation is always used")
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let key = invocation.arguments()["key"].as_str().unwrap().to_owned();
        let resource = invocation.arguments()["resource"]
            .as_str()
            .unwrap()
            .to_owned();
        let capabilities = (key == "approval").then(|| {
            CapabilityRequest::network(
                NetworkTarget::ToolManaged,
                CapabilitySource::built_in_tool("approved"),
            )
        });
        let gate = self.probe.gate(&key);
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                [ToolResourceAccess::exclusive(ToolResource::opaque(
                    "approval-test",
                    resource,
                ))],
                capabilities,
                ToolMetadata::new(),
                move |context| {
                    Box::pin(async move {
                        self.probe.started.send(key.clone()).unwrap();
                        tokio::select! {
                            permit = gate.acquire() => permit.unwrap().forget(),
                            () = context.cancellation().cancelled() => {
                                return Err(ToolError::cancelled());
                            }
                        }
                        Ok(ToolOutput::text(key))
                    })
                },
            ))
        })
    }
}

#[tokio::test]
async fn multiple_host_input_requests_are_correlated_and_answered_once() {
    let scripted = provider(vec![
        call("first-call", "ask", "first", "a"),
        call("second-call", "ask", "second", "b"),
    ]);
    let runtime = Rho::builder()
        .provider(scripted.clone())
        .tool(HostInputTool {
            shared_request: None,
            exclusive_resource: false,
        })
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("ask twice")).await.unwrap();
    let mut pending = BTreeMap::new();

    while pending.len() < 2 {
        if let RunEvent::ToolHostInputRequested { call_id, request } =
            run.next_event().await.unwrap()
        {
            pending.insert(call_id.to_string(), request);
        }
    }
    assert_eq!(pending.len(), 2);
    for (call_id, request) in &pending {
        run.respond(
            request.id().clone(),
            HostInputResponse::new().answer("answer", ["yes"]),
        )
        .await
        .unwrap_or_else(|error| panic!("response for {call_id} failed: {error}"));
        assert!(run
            .respond(
                request.id().clone(),
                HostInputResponse::new().answer("answer", ["yes"]),
            )
            .await
            .is_err());
    }
    while run.next_event().await.is_some() {}
    assert_eq!(run.outcome().await.unwrap().text(), "done");
    let requests = scripted.recorded_requests();
    let results = requests[1]
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result.content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results, ["yes", "yes"]);
}

#[tokio::test]
async fn duplicate_host_input_ids_fail_closed_without_losing_the_first_request() {
    let duplicate = question_request("duplicate");
    let runtime = Rho::builder()
        .provider(provider(vec![
            call("first-call", "ask", "first", "a"),
            call("second-call", "ask", "second", "b"),
        ]))
        .tool(HostInputTool {
            shared_request: Some(duplicate.clone()),
            exclusive_resource: false,
        })
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start(UserInput::text("duplicate IDs"))
        .await
        .unwrap();
    let mut requests = Vec::new();
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolHostInputRequested { request, .. } = event {
            requests.push(request);
        }
    }
    assert_eq!(requests.len(), 1);
    assert!(run.outcome().await.is_err());
}

#[tokio::test]
async fn duplicate_host_input_id_reused_after_answering_first_request_still_fails_closed() {
    let duplicate = question_request("duplicate");
    let runtime = Rho::builder()
        .provider(provider(vec![
            call("first-call", "ask", "serial", "a"),
            call("second-call", "ask", "serial", "b"),
        ]))
        .tool(HostInputTool {
            shared_request: Some(duplicate),
            exclusive_resource: true,
        })
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start(UserInput::text("reuse answered ID"))
        .await
        .unwrap();

    let first_request = loop {
        if let RunEvent::ToolHostInputRequested { call_id, request } =
            tokio::time::timeout(TEST_TIMEOUT, run.next_event())
                .await
                .expect("first host input request was not emitted")
                .expect("run ended before requesting host input")
        {
            assert_eq!(call_id.as_str(), "first-call");
            break request;
        }
    };
    run.respond(
        first_request.id().clone(),
        HostInputResponse::new().answer("answer", ["yes"]),
    )
    .await
    .unwrap();

    let mut request_count = 1;
    while let Some(event) = run.next_event().await {
        request_count += matches!(event, RunEvent::ToolHostInputRequested { .. }) as usize;
    }
    assert_eq!(request_count, 1);
    assert!(matches!(
        run.outcome().await,
        Err(Error::InvalidHostResponse { .. })
    ));
}

#[tokio::test]
async fn final_progress_sent_immediately_before_return_precedes_tool_finished() {
    let runtime = Rho::builder()
        .provider(provider(vec![call(
            "progress-call",
            "final_progress",
            "unused",
            "unused",
        )]))
        .tool(FinalProgressTool)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("progress")).await.unwrap();
    let mut progress_index = None;
    let mut finished_index = None;
    let mut index = 0;

    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::ToolUpdated { call_id, progress } if call_id.as_str() == "progress-call" => {
                assert_eq!(progress.text(), "final update");
                progress_index = Some(index);
            }
            RunEvent::ToolFinished { call_id, .. } if call_id.as_str() == "progress-call" => {
                finished_index = Some(index);
            }
            _ => {}
        }
        index += 1;
    }

    assert!(progress_index.unwrap() < finished_index.unwrap());
    assert_eq!(run.outcome().await.unwrap().text(), "done");
}

#[tokio::test]
async fn respond_and_steering_are_acknowledged_while_progress_backpressures_events() {
    let request = question_request("backpressure");
    let progress_gate = Arc::new(Semaphore::new(0));
    let finish_gate = Arc::new(Semaphore::new(0));
    let (progress_sent, mut sent) = mpsc::unbounded_channel();
    let runtime = Rho::builder()
        .provider(provider(vec![
            call("ask-call", "interact", "ask", "ask"),
            call("progress-call", "interact", "progress", "progress"),
        ]))
        .tool(BackpressureInteractionTool {
            request,
            progress_gate: Arc::clone(&progress_gate),
            progress_sent,
            finish_gate: Arc::clone(&finish_gate),
        })
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start(UserInput::text("backpressure"))
        .await
        .unwrap();

    let pending = loop {
        if let RunEvent::ToolHostInputRequested { request, .. } =
            tokio::time::timeout(TEST_TIMEOUT, run.next_event())
                .await
                .expect("host input request was not delivered")
                .expect("run ended before requesting host input")
        {
            break request;
        }
    };

    progress_gate.add_permits(2);
    for _ in 0..2 {
        tokio::time::timeout(TEST_TIMEOUT, sent.recv())
            .await
            .expect("progress sender stalled")
            .expect("progress sender closed");
    }

    tokio::time::timeout(
        TEST_TIMEOUT,
        run.respond(
            pending.id().clone(),
            HostInputResponse::new().answer("answer", ["yes"]),
        ),
    )
    .await
    .expect("respond was not acknowledged under event backpressure")
    .unwrap();
    let steering = tokio::time::timeout(
        TEST_TIMEOUT,
        run.steer_retractable(UserInput::text("steered under backpressure")),
    )
    .await
    .expect("steering was not acknowledged under event backpressure")
    .unwrap();
    assert!(!steering.as_str().is_empty());
    tokio::time::timeout(TEST_TIMEOUT, run.retract_steering(steering))
        .await
        .expect("steering retraction was not acknowledged under event backpressure")
        .unwrap();

    finish_gate.add_permits(1);
    while run.next_event().await.is_some() {}
    assert_eq!(run.outcome().await.unwrap().text(), "done");
}

#[tokio::test]
async fn steering_during_parallel_batch_applies_after_all_results_close() {
    let (probe, mut started) = InteractionProbe::new();
    let scripted = provider(vec![
        call("first", "work", "first", "a"),
        call("second", "work", "second", "b"),
    ]);
    let runtime = Rho::builder()
        .provider(scripted.clone())
        .tool(CancellableTool {
            probe: Arc::clone(&probe),
        })
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();
    next_started(&mut started).await;
    next_started(&mut started).await;

    let steering = run
        .steer_retractable(UserInput::text("steered"))
        .await
        .unwrap();
    assert!(!steering.as_str().is_empty());
    probe.release("second");
    probe.release("first");
    while run.next_event().await.is_some() {}
    assert_eq!(run.outcome().await.unwrap().text(), "done");

    let requests = scripted.recorded_requests();
    let messages = &requests[1].messages;
    let result_indexes = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| matches!(message, Message::ToolResult(_)).then_some(index))
        .collect::<Vec<_>>();
    let steering_index = messages
        .iter()
        .position(|message| {
            matches!(
                message,
                Message::User(content)
                    if matches!(content.as_slice(), [ContentBlock::Text(text)] if text == "steered")
            )
        })
        .unwrap();
    assert!(result_indexes.iter().all(|index| *index < steering_index));
}

#[tokio::test]
async fn concurrent_pending_preparations_cancel_into_every_result_slot() {
    let (started, mut preparation_started) = mpsc::unbounded_channel();
    let runtime = Rho::builder()
        .provider(provider(vec![
            call("preparing", "pending_prepare", "one", "a"),
            call("not-prepared-one", "pending_prepare", "two", "b"),
            call("not-prepared-two", "pending_prepare", "three", "c"),
        ]))
        .tool(PendingPreparationTool { started })
        .max_parallel_tools(NonZeroUsize::new(3).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start(UserInput::text("cancel preparation"))
        .await
        .unwrap();
    for _ in 0..3 {
        tokio::time::timeout(TEST_TIMEOUT, preparation_started.recv())
            .await
            .expect("all tool preparations did not start concurrently")
            .expect("preparation probe closed");
    }
    let mut proposed = 0;
    while proposed < 3 {
        let event = tokio::time::timeout(TEST_TIMEOUT, run.next_event())
            .await
            .expect("tool proposals were blocked by pending preparation")
            .expect("run ended before proposing every tool call");
        if matches!(event, RunEvent::ToolProposed { .. }) {
            proposed += 1;
        }
    }

    run.cancel();
    while run.next_event().await.is_some() {}
    assert!(matches!(run.outcome().await, Err(Error::Cancelled)));

    let results = session
        .history()
        .into_iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 3);
    assert_eq!(
        results
            .iter()
            .map(|result| result.id.as_str())
            .collect::<Vec<_>>(),
        ["preparing", "not-prepared-one", "not-prepared-two"]
    );
    assert!(results.iter().all(|result| !result.ok));
    assert!(results
        .iter()
        .all(|result| result.content == "tool call interrupted before completion"));
}

#[tokio::test]
async fn cancellation_cleans_up_running_and_queued_workers() {
    let (probe, mut started) = InteractionProbe::new();
    let runtime = Rho::builder()
        .provider(provider(vec![
            call("running", "work", "running", "a"),
            call("queued-one", "work", "queued-one", "b"),
            call("queued-two", "work", "queued-two", "c"),
        ]))
        .tool(CancellableTool { probe })
        .max_parallel_tools(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("cancel")).await.unwrap();
    assert_eq!(next_started(&mut started).await, "running");

    run.cancel();
    while run.next_event().await.is_some() {}
    assert!(matches!(run.outcome().await, Err(Error::Cancelled)));
    assert!(started.try_recv().is_err());
    assert!(!session.is_running());
}

#[tokio::test]
async fn dropping_event_consumer_cleans_up_all_active_batch_workers() {
    let (probe, mut started) = InteractionProbe::new();
    let runtime = Rho::builder()
        .provider(provider(vec![
            call("first", "work", "first", "a"),
            call("second", "work", "second", "b"),
        ]))
        .tool(CancellableTool { probe })
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let run = session
        .start(UserInput::text("drop consumer"))
        .await
        .unwrap();
    next_started(&mut started).await;
    next_started(&mut started).await;

    drop(run);
    tokio::time::timeout(TEST_TIMEOUT, async {
        while session.is_running() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropping the run left batch workers registered");
}

#[tokio::test]
async fn cancellation_cleans_up_parallel_host_input_waiters() {
    let runtime = Rho::builder()
        .provider(provider(vec![
            call("first", "ask", "first", "a"),
            call("second", "ask", "second", "b"),
        ]))
        .tool(HostInputTool {
            shared_request: None,
            exclusive_resource: false,
        })
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start(UserInput::text("cancel input"))
        .await
        .unwrap();
    let mut seen = 0;
    while seen < 2 {
        seen += matches!(
            run.next_event().await,
            Some(RunEvent::ToolHostInputRequested { .. })
        ) as usize;
    }
    run.cancel();
    while run.next_event().await.is_some() {}
    assert!(matches!(run.outcome().await, Err(Error::Cancelled)));
    assert!(!session.is_running());
}

#[tokio::test]
async fn pending_approval_allows_unrelated_work_but_blocks_conflicting_work() {
    let (probe, mut started) = InteractionProbe::new();
    let (handler, mut approvals) = approval_channel(NonZeroUsize::new(2).unwrap());
    let runtime = Rho::builder()
        .provider(provider(vec![
            call("approval", "approved", "approval", "same"),
            call("unrelated", "approved", "unrelated", "other"),
            call("conflict", "approved", "conflict", "same"),
        ]))
        .tool(ApprovalTool {
            probe: Arc::clone(&probe),
        })
        .workspace_policy(
            ScopedWorkspacePolicy::new()
                .allow_network_tool("approved")
                .require_network_approval(),
        )
        .approval_handler(handler)
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let run = session.start(UserInput::text("approve")).await.unwrap();

    let mut pending = tokio::time::timeout(TEST_TIMEOUT, approvals.recv())
        .await
        .expect("approval was not requested")
        .expect("approval channel closed");
    assert_eq!(next_started(&mut started).await, "unrelated");
    probe.release("unrelated");
    pending.respond(ApprovalDecision::AllowOnce).unwrap();
    assert_eq!(next_started(&mut started).await, "approval");
    probe.release("approval");
    assert_eq!(next_started(&mut started).await, "conflict");
    probe.release("conflict");
    let mut run = run;
    while run.next_event().await.is_some() {}
    assert_eq!(run.outcome().await.unwrap().text(), "done");
}

#[tokio::test]
async fn cancellation_while_authorizing_prevents_all_workers_from_starting() {
    let (probe, mut started) = InteractionProbe::new();
    let (handler, mut approvals) = approval_channel(NonZeroUsize::new(1).unwrap());
    let runtime = Rho::builder()
        .provider(provider(vec![call(
            "approval", "approved", "approval", "same",
        )]))
        .tool(ApprovalTool { probe })
        .workspace_policy(
            ScopedWorkspacePolicy::new()
                .allow_network_tool("approved")
                .require_network_approval(),
        )
        .approval_handler(handler)
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start(UserInput::text("cancel approval"))
        .await
        .unwrap();
    let mut pending = approvals.recv().await.unwrap();

    run.cancel();
    let _ = pending.respond(ApprovalDecision::AllowOnce);
    while run.next_event().await.is_some() {}
    assert!(matches!(run.outcome().await, Err(Error::Cancelled)));
    assert!(started.try_recv().is_err());
}
