use std::{
    io,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    tool::{
        PreparedToolInvocation, ScriptedTool, ScriptedToolOutcome, Tool, ToolContext, ToolError,
        ToolErrorKind, ToolFuture, ToolInvocation, ToolMetadata, ToolOutput,
        ToolPreparationContext, ToolPrepareFuture, ToolProgress, ToolResource, ToolResourceAccess,
    },
    CancellationToken, HostChoice, HostInputRequest, HostInputResponse, HostQuestion, Rho,
    SelectionMode, SessionOptions, SystemPrompt, Workspace,
};
use serde_json::json;
use tokio::{
    sync::{oneshot, Semaphore},
    time::timeout,
};

use super::{
    classify_error, complete_run, prompt_from_reader, AutomationExit, RunArtifactIdentity,
    RunReporter,
};
use crate::app::headless_run::{HeadlessRunDeps, HostInputRespondFuture, HostInputResponder};
use crate::{
    app::{
        automation_protocol::TerminalReason,
        policy::AppPolicy,
        runtime_builder::{build_runtime, RuntimeBuildOptions},
    },
    compaction::CompactionConfig,
    permission::PermissionMode,
};

#[test]
fn classifies_automation_exit_without_parsing_its_message() {
    let error = anyhow::Error::new(AutomationExit::new(
        1,
        TerminalReason::OutputError,
        "wording can change",
    ));

    assert_eq!(classify_error(&error), (TerminalReason::OutputError, 1));
}

#[test]
fn classifies_fatal_tool_host_errors() {
    let error = anyhow::Error::new(rho_sdk::Error::Tool(rho_sdk::tool::ToolError::new(
        rho_sdk::tool::ToolErrorKind::Execution,
        "host failed",
    )));

    assert_eq!(classify_error(&error), (TerminalReason::ToolHostError, 1));
}

#[test]
fn reporter_discards_partial_text_when_provider_attempt_resets() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("result.json");
    let mut reporter = RunReporter::new(
        output,
        RunArtifactIdentity {
            agent_id: "reviewer".into(),
            agent_fingerprint: "fingerprint".into(),
            provider: "test".into(),
            model: "test".into(),
        },
        root.path().to_path_buf(),
        "review",
        /* stream_output */ false,
        None,
    )
    .unwrap();

    reporter.on_event(&rho_sdk::RunEvent::AssistantTextDelta {
        text: "stale partial response".into(),
    });
    reporter.on_event(&rho_sdk::RunEvent::ProviderStreamReset {
        reason: rho_sdk::ProviderStreamResetReason::RetryableFailure(
            rho_sdk::ProviderErrorKind::Unavailable,
        ),
        detail: "retrying".into(),
    });

    assert_eq!(reporter.status().last_text, None);
    assert_eq!(
        reporter.status().last_activity.as_deref(),
        Some("retrying provider response")
    );
}

#[test]
fn prompt_requires_input() {
    let mut stdin = io::empty();
    let error = prompt_from_reader(Vec::new(), /*read_stdin*/ false, &mut stdin).unwrap_err();

    assert!(error.to_string().contains("requires a prompt"));
}

#[tokio::test]
async fn headless_run_compacts_at_configured_threshold_and_completes() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "automation-compaction"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: "expand_context".into(),
                    arguments: json!({}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "compact summary".into(),
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let shared_provider: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(ScriptedTool::new(
        ToolSpec {
            name: "expand_context".into(),
            description: "return a large result".into(),
            input_schema: json!({"type": "object"}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text("tool context ".repeat(500))),
    ))];
    let root = tempfile::tempdir().unwrap();
    let runtime = build_runtime(RuntimeBuildOptions {
        provider: shared_provider,
        tools: &tools,
        workspace: Workspace::new(root.path()).unwrap(),
        workspace_policy: AppPolicy::for_mode(PermissionMode::Auto),
        approval_handler: None,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Off,
        service_tier: None,
        compaction: CompactionConfig {
            auto_compact: true,
            threshold_percent: 5,
            target_percent: 1,
        },
        context_window: Some(1_000),
        usage_purpose: "agent",
        usage_parent_session_id: None,
        usage_recording: Default::default(),
        hooks: None,
    })
    .unwrap();
    assert_eq!(runtime.diagnostics().compaction_trigger_tokens(), Some(50));
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let outcome = complete_run(
        &session,
        "continue".into(),
        HeadlessRunDeps {
            reporter: None,
            external_cancellation: None,
            jsonl: None,
            host_input: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(outcome.text(), "done");
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].messages.iter().any(|message| {
        matches!(
            message,
            Message::User(blocks)
                if blocks.iter().any(|block| matches!(
                    block,
                    ContentBlock::Text(text)
                        if text.starts_with("Automatic compaction summary")
                ))
        )
    }));
    runtime.shutdown();
}

fn host_question_request(title: &str) -> HostInputRequest {
    let question = HostQuestion::new(
        "answer",
        "choose",
        vec![HostChoice::new("yes", "yes")],
        SelectionMode::One,
    )
    .unwrap();
    HostInputRequest::questionnaire(title, vec![question]).unwrap()
}

fn assistant_calls(calls: &[(&str, &str, &str)]) -> ModelResponse {
    ModelResponse::Assistant(
        calls
            .iter()
            .map(|(id, name, key)| {
                ContentBlock::ToolCall(ToolCall {
                    id: (*id).into(),
                    name: (*name).into(),
                    arguments: json!({ "key": key }),
                })
            })
            .collect(),
    )
}

#[derive(Clone)]
struct ImmediateHostInputResponder {
    answer: &'static str,
}

impl HostInputResponder for ImmediateHostInputResponder {
    fn respond<'a>(
        &'a self,
        _request: HostInputRequest,
        _cancellation: &'a CancellationToken,
    ) -> HostInputRespondFuture<'a> {
        let answer = self.answer;
        Box::pin(async move { Ok(HostInputResponse::new().answer("answer", [answer])) })
    }
}

#[derive(Clone)]
struct GatedHostInputResponder {
    release: Arc<Semaphore>,
    waiting: Arc<AtomicUsize>,
}

impl HostInputResponder for GatedHostInputResponder {
    fn respond<'a>(
        &'a self,
        _request: HostInputRequest,
        cancellation: &'a CancellationToken,
    ) -> HostInputRespondFuture<'a> {
        let release = Arc::clone(&self.release);
        let waiting = Arc::clone(&self.waiting);
        Box::pin(async move {
            waiting.fetch_add(1, Ordering::AcqRel);
            tokio::select! {
                permit = release.acquire() => {
                    permit
                        .map_err(|_| rho_sdk::Error::Interrupted {
                            message: "host input gate closed".into(),
                        })?
                        .forget();
                    Ok(HostInputResponse::new().answer("answer", ["yes"]))
                }
                () = cancellation.cancelled() => Err(rho_sdk::Error::Cancelled),
            }
        })
    }
}

#[derive(Clone)]
struct HeadlessHostInputTool {
    request: HostInputRequest,
    progress_gate: Arc<Semaphore>,
    progress_sent: Arc<AtomicUsize>,
    finish_gate: Arc<Semaphore>,
}

impl Tool for HeadlessHostInputTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "interact".into(),
            description: "headless host-input backpressure probe".into(),
            input_schema: json!({"type": "object"}),
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
                    "headless-backpressure-test",
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
                                    self.progress_sent.fetch_add(1, Ordering::AcqRel);
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

#[tokio::test]
async fn headless_run_fails_closed_without_host_input_responder() {
    let request = host_question_request("fail-closed");
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "fail-closed"),
            [ScriptedTurn::completed(assistant_calls(&[(
                "ask-call", "interact", "ask",
            )]))],
        ))
        .tool(HeadlessHostInputTool {
            request,
            progress_gate: Arc::new(Semaphore::new(0)),
            progress_sent: Arc::new(AtomicUsize::new(0)),
            finish_gate: Arc::new(Semaphore::new(0)),
        })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let error = timeout(
        Duration::from_secs(2),
        complete_run(
            &session,
            "ask".into(),
            HeadlessRunDeps {
                reporter: None,
                external_cancellation: None,
                jsonl: None,
                host_input: None,
            },
        ),
    )
    .await
    .expect("fail-closed path should finish")
    .unwrap_err();

    assert!(
        error.to_string().contains("cannot answer host input"),
        "unexpected error: {error}"
    );
    runtime.shutdown();
}

#[tokio::test]
async fn headless_run_answers_host_input_through_generic_responder() {
    let request = host_question_request("generic-responder");
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "generic-responder"),
            [
                ScriptedTurn::completed(assistant_calls(&[("ask-call", "interact", "ask")])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ],
        ))
        .tool(HeadlessHostInputTool {
            request,
            progress_gate: Arc::new(Semaphore::new(0)),
            progress_sent: Arc::new(AtomicUsize::new(0)),
            finish_gate: Arc::new(Semaphore::new(0)),
        })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let responder = ImmediateHostInputResponder { answer: "yes" };

    let outcome = timeout(
        Duration::from_secs(2),
        complete_run(
            &session,
            "ask".into(),
            HeadlessRunDeps {
                reporter: None,
                external_cancellation: None,
                jsonl: None,
                host_input: Some(&responder as &dyn HostInputResponder),
            },
        ),
    )
    .await
    .expect("generic responder path should finish")
    .unwrap();

    assert_eq!(outcome.text(), "done");
    runtime.shutdown();
}

#[tokio::test]
async fn headless_run_drains_events_while_waiting_for_parent_host_input() {
    let request = host_question_request("drain-while-waiting");
    let progress_gate = Arc::new(Semaphore::new(0));
    let finish_gate = Arc::new(Semaphore::new(0));
    let progress_sent = Arc::new(AtomicUsize::new(0));
    let parent_release = Arc::new(Semaphore::new(0));
    let parent_waiting = Arc::new(AtomicUsize::new(0));
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "drain-while-waiting"),
            [
                ScriptedTurn::completed(assistant_calls(&[
                    ("ask-call", "interact", "ask"),
                    ("progress-call", "interact", "progress"),
                ])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ],
        ))
        .tool(HeadlessHostInputTool {
            request,
            progress_gate: Arc::clone(&progress_gate),
            progress_sent: Arc::clone(&progress_sent),
            finish_gate: Arc::clone(&finish_gate),
        })
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let responder = GatedHostInputResponder {
        release: Arc::clone(&parent_release),
        waiting: Arc::clone(&parent_waiting),
    };

    let drive = tokio::spawn({
        let session = session;
        async move {
            complete_run(
                &session,
                "ask".into(),
                HeadlessRunDeps {
                    reporter: None,
                    external_cancellation: None,
                    jsonl: None,
                    host_input: Some(&responder as &dyn HostInputResponder),
                },
            )
            .await
        }
    });

    timeout(Duration::from_secs(2), async {
        while parent_waiting.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent responder should start waiting");

    progress_gate.add_permits(1);
    timeout(Duration::from_secs(2), async {
        while progress_sent.load(Ordering::Acquire) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("progress should keep flowing while the parent answer is pending");

    parent_release.add_permits(1);
    finish_gate.add_permits(1);

    let outcome = timeout(Duration::from_secs(2), drive)
        .await
        .expect("headless drain path should finish")
        .expect("drive task")
        .expect("run outcome");
    assert_eq!(outcome.text(), "done");
    runtime.shutdown();
}

#[tokio::test]
async fn headless_run_drains_events_while_waiting_for_respond_ack() {
    let request = host_question_request("drain-while-ack");
    let progress_gate = Arc::new(Semaphore::new(0));
    let finish_gate = Arc::new(Semaphore::new(0));
    let progress_sent = Arc::new(AtomicUsize::new(0));
    let (hold_tx, hold_rx) = oneshot::channel::<()>();
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "drain-while-ack"),
            [
                ScriptedTurn::completed(assistant_calls(&[
                    ("ask-call", "interact", "ask"),
                    ("progress-call", "interact", "progress"),
                ])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ],
        ))
        .tool(HeadlessHostInputTool {
            request,
            progress_gate: Arc::clone(&progress_gate),
            progress_sent: Arc::clone(&progress_sent),
            finish_gate: Arc::clone(&finish_gate),
        })
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    // Answer immediately so drive_headless_run spends time on the Respond ack path.
    struct ImmediateThenSignal {
        hold: std::sync::Mutex<Option<oneshot::Sender<()>>>,
    }
    impl HostInputResponder for ImmediateThenSignal {
        fn respond<'a>(
            &'a self,
            _request: HostInputRequest,
            _cancellation: &'a CancellationToken,
        ) -> HostInputRespondFuture<'a> {
            let hold = self.hold.lock().expect("hold lock").take();
            Box::pin(async move {
                if let Some(hold) = hold {
                    let _ = hold.send(());
                }
                Ok(HostInputResponse::new().answer("answer", ["yes"]))
            })
        }
    }
    let responder = ImmediateThenSignal {
        hold: std::sync::Mutex::new(Some(hold_tx)),
    };

    let drive = tokio::spawn(async move {
        complete_run(
            &session,
            "ask".into(),
            HeadlessRunDeps {
                reporter: None,
                external_cancellation: None,
                jsonl: None,
                host_input: Some(&responder as &dyn HostInputResponder),
            },
        )
        .await
    });

    timeout(Duration::from_secs(2), hold_rx)
        .await
        .expect("responder should answer")
        .expect("hold signal");

    progress_gate.add_permits(2);
    timeout(Duration::from_secs(2), async {
        while progress_sent.load(Ordering::Acquire) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("progress should keep flowing while respond ack is pending");

    finish_gate.add_permits(1);
    let outcome = timeout(Duration::from_secs(2), drive)
        .await
        .expect("ack drain path should finish")
        .expect("drive task")
        .expect("run outcome");
    assert_eq!(outcome.text(), "done");
    runtime.shutdown();
}
