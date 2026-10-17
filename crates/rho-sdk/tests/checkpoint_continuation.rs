mod support;

use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{
        ContentBlock, Message, ModelEvent, ModelResponse, ProviderContextBlock, ToolCall,
        ToolResult, ToolSpec,
    },
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{
        PreparedToolInvocation, Tool, ToolContext, ToolExecutionMode, ToolFuture, ToolInvocation,
        ToolOutput, ToolPreparationContext, ToolPrepareFuture, ToolSecurity,
    },
    CapabilityKind, CompactionFuture, CompactionOutput, CompactionPolicy, CompactionRequest,
    Compactor, InMemorySessionStore, Rho, RunEvent, SessionId, SessionOptions, SessionSnapshot,
    SessionStore, SessionStoreFuture, UserInput,
};
use serde_json::json;
use tokio::sync::{mpsc, Notify};

use support::{identity, text_response, TEST_TIMEOUT};

/// Records each invocation by name. A stalled tool never finishes on its own.
struct RecordingTool {
    name: &'static str,
    capability: CapabilityKind,
    invoked: mpsc::UnboundedSender<&'static str>,
    stall: bool,
}

impl Tool for RecordingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "recording test tool".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([self.capability])
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        self.invoked.send(self.name).unwrap();
        Box::pin(async move {
            if self.stall {
                std::future::pending::<()>().await;
            }
            Ok(ToolOutput::text(format!("{} again", self.name)))
        })
    }
}

fn runtime(
    provider: ScriptedProvider,
    invoked: &mpsc::UnboundedSender<&'static str>,
    stall: bool,
) -> Rho {
    let tool = |name, capability| RecordingTool {
        name,
        capability,
        invoked: invoked.clone(),
        stall,
    };
    Rho::builder()
        .provider(provider)
        .tool(tool("read", CapabilityKind::Read))
        .tool(tool("write", CapabilityKind::Write))
        .build()
        .unwrap()
}

fn call(id: &str, name: &str) -> ContentBlock {
    ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: json!({}),
    })
}

// Covers: a run that dies after a model reply leaves a checkpoint naming the
// reply's tool calls. Continuing it reruns only the read-only call, fails the
// other back to the model, and asks the model again. Owner: SDK checkpoints.
#[tokio::test]
async fn continuation_reruns_read_only_calls_and_interrupts_the_rest() {
    let store = Arc::new(InMemorySessionStore::new());
    let (invoked, mut invocations) = mpsc::unbounded_channel();
    let crashed = runtime(
        ScriptedProvider::new(
            identity(),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                call("read-1", "read"),
                call("write-1", "write"),
            ]))],
        ),
        &invoked,
        /* stall */ true,
    );
    let session = crashed.session(SessionOptions::default()).await.unwrap();
    session.set_checkpoint_store(Some(store.clone())).unwrap();
    let run = session.start(UserInput::text("inspect")).await.unwrap();
    tokio::time::timeout(TEST_TIMEOUT, invocations.recv())
        .await
        .expect("no tool started")
        .unwrap();
    // A dropped event consumer stops the run without a terminal commit, so the
    // store keeps exactly what a crash would leave behind.
    drop(run);
    let saved = store.load(session.id()).expect("checkpoint was saved");
    assert_eq!(
        saved
            .history()
            .last()
            .and_then(Message::completed_assistant_content),
        Some([call("read-1", "read"), call("write-1", "write")].as_slice())
    );

    let provider =
        ScriptedProvider::new(identity(), [ScriptedTurn::completed(text_response("done"))]);
    let (invoked, mut invocations) = mpsc::unbounded_channel();
    let resumed = runtime(provider.clone(), &invoked, /* stall */ false);
    let session = resumed
        .session(SessionOptions::from_snapshot(saved.clone()))
        .await
        .unwrap();
    let mut run = session.continue_history().await.unwrap();
    while run.next_event().await.is_some() {}
    assert_eq!(run.outcome().await.unwrap().text(), "done");

    let mut rerun = Vec::new();
    while let Ok(name) = invocations.try_recv() {
        rerun.push(name);
    }
    assert_eq!(rerun, ["read"]);
    let mut expected = saved.history().to_vec();
    expected.extend([
        Message::ToolResult(ToolResult {
            id: "write-1".into(),
            ok: false,
            content: "tool call interrupted before completion".into(),
        }),
        Message::ToolResult(ToolResult {
            id: "read-1".into(),
            ok: true,
            content: "read again".into(),
        }),
    ]);
    assert_eq!(provider.recorded_requests()[0].messages, expected);
}

// Covers: a run that dies while a host batch runs leaves a checkpoint naming
// the user input and every host call, so continuing it settles those calls
// instead of losing the turn. Owner: SDK checkpoints.
#[tokio::test]
async fn host_batch_is_checkpointed_before_its_calls_start() {
    let store = Arc::new(InMemorySessionStore::new());
    let (invoked, mut invocations) = mpsc::unbounded_channel();
    let crashed = runtime(
        ScriptedProvider::new(identity(), Vec::<ScriptedTurn>::new()),
        &invoked,
        /* stall */ true,
    );
    let session = crashed.session(SessionOptions::default()).await.unwrap();
    session.set_checkpoint_store(Some(store.clone())).unwrap();
    let calls = ["read", "write"].map(|name| ToolCall {
        id: format!("host-{name}"),
        name: name.into(),
        arguments: json!({}),
    });
    let run = session
        .start_with_tool_calls(UserInput::text("load both"), calls.to_vec())
        .await
        .unwrap();
    tokio::time::timeout(TEST_TIMEOUT, invocations.recv())
        .await
        .expect("no host call started")
        .unwrap();
    drop(run);

    let saved = store.load(session.id()).expect("checkpoint was saved");
    assert_eq!(
        saved.history(),
        [
            Message::user_text("load both"),
            Message::Assistant(calls.map(ContentBlock::ToolCall).to_vec()),
        ]
    );
}

/// Keeps the history of every snapshot it is asked to save.
#[derive(Default)]
struct RecordingStore(Mutex<Vec<Vec<Message>>>);

impl RecordingStore {
    fn saved(&self) -> Vec<Vec<Message>> {
        self.0.lock().unwrap().clone()
    }
}

impl SessionStore for RecordingStore {
    fn load<'a>(&'a self, _id: &'a SessionId) -> SessionStoreFuture<'a, Option<SessionSnapshot>> {
        Box::pin(async { Ok(None) })
    }

    fn save<'a>(&'a self, snapshot: SessionSnapshot) -> SessionStoreFuture<'a, ()> {
        self.0.lock().unwrap().push(snapshot.history().to_vec());
        Box::pin(async { Ok(()) })
    }
}

/// Summarizes all but the latest exchange, which it keeps verbatim. The
/// replacement can then be longer than the history the run last saved.
#[derive(Clone, Default)]
struct SummaryCompactor(Arc<Mutex<usize>>);

impl Compactor for SummaryCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        let mut calls = self.0.lock().unwrap();
        *calls += 1;
        let messages = request.messages();
        let mut replacement = vec![Message::user_text(format!("summary {calls}"))];
        replacement.extend_from_slice(&messages[messages.len().saturating_sub(2)..]);
        Box::pin(async move { CompactionOutput::new(replacement) })
    }
}

// Covers: a checkpoint copies only the messages appended since the run's last
// commit, so a compaction that rewrites history mid-run must still reach the
// store whole. Each checkpoint before a request equals what the provider then
// receives, and each checkpoint after a reply adds exactly that reply.
// Owner: SDK checkpoints.
#[tokio::test]
async fn checkpoints_match_each_request_across_compaction() {
    let replies = [
        vec![call("read-1", "read")],
        vec![call("read-2", "read")],
        vec![ContentBlock::Text("done".into())],
    ];
    let provider = ScriptedProvider::new(
        identity(),
        replies
            .clone()
            .map(|reply| ScriptedTurn::completed(ModelResponse::Assistant(reply))),
    );
    let (invoked, _invocations) = mpsc::unbounded_channel();
    let compactor = SummaryCompactor::default();
    let runtime = Rho::builder()
        .provider(provider.clone())
        .tool(RecordingTool {
            name: "read",
            capability: CapabilityKind::Read,
            invoked,
            stall: false,
        })
        .compactor(compactor.clone())
        .compaction_policy(CompactionPolicy::after_messages(
            NonZeroUsize::new(2).unwrap(),
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let store = Arc::new(RecordingStore::default());
    session.set_checkpoint_store(Some(store.clone())).unwrap();

    let outcome = tokio::time::timeout(TEST_TIMEOUT, session.complete("read twice"))
        .await
        .expect("run stalled")
        .unwrap();
    assert_eq!(outcome.text(), "done");
    assert!(*compactor.0.lock().unwrap() > 0, "no compaction ran");

    let requests = provider.recorded_requests();
    let saved = store.saved();
    assert_eq!(saved.len(), 2 * requests.len());
    for ((request, reply), saves) in requests.iter().zip(&replies).zip(saved.chunks(2)) {
        let [before, after] = saves else {
            unreachable!("chunks of two")
        };
        assert_eq!(before, &request.messages);
        let (last, prefix) = after.split_last().unwrap();
        assert_eq!(prefix, request.messages.as_slice());
        assert_eq!(last.completed_assistant_content(), Some(reply.as_slice()));
    }
}

/// An async tool whose job finishes only when the test releases it.
struct GatedAsyncTool(Arc<Notify>);

impl Tool for GatedAsyncTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "slow".into(),
            description: "gated async test tool".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Async
    }

    fn prepare<'a>(
        &'a self,
        _invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let gate = Arc::clone(&self.0);
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                Default::default(),
                move |_context| {
                    Box::pin(async move {
                        gate.notified().await;
                        Ok(ToolOutput::text("slow done"))
                    })
                },
            ))
        })
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        unreachable!("async tool runs through prepare")
    }
}

// Covers: while an async job runs, the next provider request has nothing new
// to save, so the run does not write the same checkpoint twice. Every save
// still adds the history that changed: the user's message, each reply, and
// the job's result.
// Owner: SDK checkpoints.
#[tokio::test]
async fn unchanged_history_is_not_saved_again() {
    let marker = ProviderContextBlock::async_tool_call(identity(), "call-a");
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::streaming(
                vec![ModelEvent::ProviderContext {
                    kind: marker.kind,
                    position: marker.position,
                    data: marker.data,
                }],
                ModelResponse::Assistant(vec![call("call-a", "slow")]),
            ),
            ScriptedTurn::completed(text_response("working")),
            ScriptedTurn::completed(text_response("done")),
        ],
    );
    let gate = Arc::new(Notify::new());
    let session = Rho::builder()
        .provider(provider)
        .tool(GatedAsyncTool(Arc::clone(&gate)))
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    let store = Arc::new(RecordingStore::default());
    session.set_checkpoint_store(Some(store.clone())).unwrap();

    let mut run = session.start(UserInput::text("start")).await.unwrap();
    loop {
        let event = tokio::time::timeout(TEST_TIMEOUT, run.next_event())
            .await
            .expect("run stalled")
            .expect("run ended without a terminal event");
        match event {
            // Step 2 starts after the run collected finished jobs, so the
            // result cannot land before its request.
            RunEvent::StepStarted { step: 2, .. } => gate.notify_one(),
            RunEvent::Completed { .. } => break,
            RunEvent::Failed { message, .. } => panic!("run failed: {message}"),
            _ => {}
        }
    }

    let history = session.history();
    assert_eq!(history.len(), 5, "user, call, working, result, done");
    let prefixes = (1..=history.len())
        .map(|len| history[..len].to_vec())
        .collect::<Vec<_>>();
    assert_eq!(store.saved(), prefixes);
}

// Covers: a run's first save never assumes the store already holds the
// session. Continuing a fully answered history adds nothing before the first
// request, yet the checkpoint must still reach a freshly installed store.
// Owner: SDK checkpoints.
#[tokio::test]
async fn first_checkpoint_saves_an_unchanged_restored_history() {
    let restored = vec![
        Message::user_text("inspect"),
        Message::Assistant(vec![ContentBlock::Text("looked".into())]),
    ];
    let provider =
        ScriptedProvider::new(identity(), [ScriptedTurn::completed(text_response("done"))]);
    let session = Rho::builder()
        .provider(provider)
        .build()
        .unwrap()
        .session(SessionOptions::new().history(restored.clone()))
        .await
        .unwrap();
    let store = Arc::new(RecordingStore::default());
    session.set_checkpoint_store(Some(store.clone())).unwrap();

    let mut run = session.continue_history().await.unwrap();
    while run.next_event().await.is_some() {}
    run.outcome().await.unwrap();

    assert_eq!(store.saved().first(), Some(&restored));
}
