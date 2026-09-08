mod support;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{
        ContentBlock, ImageContent, Message, ModelEvent, ModelIdentity, ModelRequest,
        ModelResponse, ProviderContextBlock, ToolCall, ToolResult, ToolSpec,
    },
    provider::{
        ModelProvider, ProviderEventSender, ProviderFuture, ScriptedProvider, ScriptedTurn,
    },
    tool::{
        PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind, ToolExecutionMode,
        ToolFuture, ToolInvocation, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
    },
    Rho, RunEvent, SessionOptions, SessionSnapshot, ToolCompletion, UserInput,
};
use serde_json::json;
use support::{identity, text_response, TEST_TIMEOUT};
use tokio::sync::Semaphore;

fn image() -> ImageContent {
    ImageContent {
        data: "aW1hZ2U=".into(),
        mime_type: "image/png".into(),
    }
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "capture".into(),
        arguments: json!({"id": id}),
    }
}

struct ImageTool {
    finished: Arc<Semaphore>,
    block_second: bool,
}

impl ImageTool {
    fn execute(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            if invocation.arguments()["id"] == "second" {
                if self.block_second {
                    std::future::pending::<()>().await;
                }
                self.finished.add_permits(1);
                Err(ToolError::new(ToolErrorKind::Execution, "capture failed"))
            } else {
                self.finished.add_permits(1);
                Ok(ToolOutput::text("captured").with_images(vec![image()]))
            }
        })
    }
}

impl Tool for ImageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "capture".into(),
            description: "image output probe".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Async
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        self.execute(invocation)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                Default::default(),
                move |_context| self.execute(invocation),
            ))
        })
    }
}

struct ImageProvider {
    async_calls: &'static [&'static str],
    fail_after_first: bool,
    turns: AtomicUsize,
    requests: Mutex<Vec<Vec<Message>>>,
    finished: Arc<Semaphore>,
}

impl ModelProvider for ImageProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async { Ok(text_response("done")) })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.requests
                .lock()
                .unwrap()
                .push(request.messages.to_vec());
            if self.turns.fetch_add(1, Ordering::SeqCst) == 0 {
                for id in self.async_calls {
                    let marker = ProviderContextBlock::async_tool_call(identity(), *id);
                    events
                        .send(ModelEvent::ProviderContext {
                            kind: marker.kind,
                            position: marker.position,
                            data: marker.data,
                        })
                        .await?;
                }
                return Ok(ModelResponse::Assistant(vec![
                    ContentBlock::ToolCall(call("first")),
                    ContentBlock::ToolCall(call("second")),
                ]));
            }
            // Two tool completions are the synchronization condition, not elapsed time.
            if self.fail_after_first {
                let permit = self.finished.acquire().await.unwrap();
                drop(permit);
                return Err(rho_sdk::ProviderError::new(
                    rho_sdk::ProviderErrorKind::Other,
                    "provider failed",
                    rho_sdk::Retryability::Permanent,
                ));
            }
            let permit = self.finished.acquire_many(2).await.unwrap();
            drop(permit);
            Ok(text_response("done"))
        })
    }
}

// Covers: current images ignore restored dangling calls, follow paired results, and survive resume.
// Owner: SDK model/history contract. Existing batch tests cover text-only results.
#[tokio::test]
async fn tool_images_follow_all_results_and_survive_resume() {
    for async_calls in [&[][..], &["first"][..], &["first", "second"][..]] {
        let finished = Arc::new(Semaphore::new(0));
        let provider = Arc::new(ImageProvider {
            async_calls,
            fail_after_first: false,
            turns: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            finished: finished.clone(),
        });
        let runtime = Rho::builder()
            .provider_shared(provider.clone())
            .tool(ImageTool {
                finished,
                block_second: false,
            })
            .build()
            .unwrap();
        let snapshot = SessionSnapshot::new(
            Default::default(),
            rho_sdk::Revision::INITIAL,
            vec![Message::Assistant(vec![ContentBlock::ToolCall(call(
                "old-dangling",
            ))])],
            identity(),
            Default::default(),
        );
        let session = runtime
            .session(SessionOptions::from_snapshot(snapshot))
            .await
            .unwrap();
        let mut run = session.start(UserInput::text("capture")).await.unwrap();
        let mut successful_outputs = Vec::new();
        tokio::time::timeout(TEST_TIMEOUT, async {
            while let Some(event) = run.next_event().await {
                if let RunEvent::ToolFinished {
                    result: ToolCompletion::Success(output),
                    ..
                } = event
                {
                    successful_outputs.push(output);
                }
            }
            run.outcome().await.unwrap();
        })
        .await
        .expect("image run did not complete");
        assert_eq!(
            successful_outputs,
            vec![ToolOutput::text("captured").with_images(vec![image()])]
        );

        {
            let requests = provider.requests.lock().unwrap();
            let last = requests.last().unwrap();
            let image_index = last
                .iter()
                .position(|message| message.as_tool_image_supplement().is_some())
                .expect("model did not receive the tool image");
            let mut results = last[..image_index]
                .iter()
                .filter_map(|message| match message {
                    Message::ToolResult(result) => Some(result.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            results.sort_by(|left, right| left.id.cmp(&right.id));
            assert_eq!(
                results,
                vec![
                    ToolResult {
                        id: "first".into(),
                        ok: true,
                        content: "captured".into()
                    },
                    ToolResult {
                        id: "second".into(),
                        ok: false,
                        content: "capture failed".into()
                    }
                ]
            );
            let supplement = last[image_index].as_tool_image_supplement().unwrap();
            assert_eq!(
                (
                    supplement.tool_name(),
                    supplement.tool_call_id(),
                    supplement.images().cloned().collect::<Vec<_>>()
                ),
                ("capture", "first", vec![image()])
            );
        }

        let snapshot: SessionSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&session.snapshot()).unwrap()).unwrap();
        let resumed_provider = Arc::new(ScriptedProvider::new(
            identity(),
            [ScriptedTurn::completed(text_response("resumed"))],
        ));
        let resumed = Rho::builder()
            .provider_shared(resumed_provider.clone())
            .build()
            .unwrap()
            .session(SessionOptions::from_snapshot(snapshot))
            .await
            .unwrap();
        let expected = session.history();
        let mut run = resumed.start(UserInput::text("continue")).await.unwrap();
        while run.next_event().await.is_some() {}
        run.outcome().await.unwrap();
        assert_eq!(
            &resumed_provider.recorded_requests()[0].messages[..expected.len()],
            expected.as_slice()
        );
    }
}

// Covers: a successful image buffered before another call is cancelled must not leak into history.
// Owner: SDK cancellation contract.
#[tokio::test]
async fn cancelled_batch_discards_undelivered_tool_images() {
    for async_calls in [&[][..], &["first"][..], &["first", "second"][..]] {
        let finished = Arc::new(Semaphore::new(0));
        let provider = ImageProvider {
            async_calls,
            fail_after_first: false,
            turns: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            finished: finished.clone(),
        };
        let session = Rho::builder()
            .provider(provider)
            .tool(ImageTool {
                finished,
                block_second: true,
            })
            .build()
            .unwrap()
            .session(SessionOptions::default())
            .await
            .unwrap();
        let mut run = session.start(UserInput::text("capture")).await.unwrap();
        tokio::time::timeout(TEST_TIMEOUT, async {
        let mut image_finished = false;
        let mut second_started = false;
        while let Some(event) = run.next_event().await {
            if matches!(event, RunEvent::ToolStarted { ref call_id, .. } if call_id.as_str() == "second") {
                second_started = true;
            }
            if matches!(event, RunEvent::ToolFinished { ref call_id, result: ToolCompletion::Success(_), .. } if call_id.as_str() == "first") {
                image_finished = true;
            }
            if image_finished && second_started {
                run.cancel();
            }
        }
        assert!(matches!(run.outcome().await, Err(rho_sdk::Error::Cancelled)));
    }).await.expect("cancelled image batch did not finish");
        let history = session.history();
        assert_eq!(
            history
                .iter()
                .filter(|message| matches!(message, Message::ToolResult(_)))
                .count(),
            2
        );
        assert!(!history.iter().any(|message| matches!(message, Message::User(content) if content.iter().any(|block| matches!(block, ContentBlock::Image(_))))));
    }
}

// Covers: terminal provider failure settles text pairing but discards undelivered images.
// Owner: SDK terminal-history contract, distinct from caller cancellation.
#[tokio::test]
async fn provider_failure_discards_buffered_images_after_settlement() {
    let finished = Arc::new(Semaphore::new(0));
    let session = Rho::builder()
        .provider(ImageProvider {
            async_calls: &["first", "second"],
            fail_after_first: true,
            turns: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            finished: finished.clone(),
        })
        .tool(ImageTool {
            finished,
            block_second: true,
        })
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    let mut run = session.start(UserInput::text("capture")).await.unwrap();
    tokio::time::timeout(TEST_TIMEOUT, async {
        while run.next_event().await.is_some() {}
        assert!(matches!(
            run.outcome().await,
            Err(rho_sdk::Error::Provider(_))
        ));
    })
    .await
    .expect("provider failure did not settle tools");
    let history = session.history();
    let results = history
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some((result.id.as_str(), result.ok)),
            _ => None,
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(results, [("first", true), ("second", false)].into());
    assert!(!history
        .iter()
        .any(|message| message.as_tool_image_supplement().is_some()));
}
