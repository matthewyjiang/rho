use std::{
    num::NonZeroUsize,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    tool::{
        PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture,
        ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
        ToolProgress, ToolResource, ToolResourceAccess,
    },
    HostChoice, HostInputRefusal, HostInputRequest, HostInputResponse, HostQuestion, Rho,
    SelectionMode, SessionId, UserInput,
};
use serde_json::json;
use tokio::{sync::Semaphore, time::timeout};

use super::{HeadlessView, ParentQuestions};
use crate::{
    app::{
        session_runtime::{
            unattended_runtime_on, QuestionSupport, SessionRuntime, Turn, TurnOutcome,
        },
        subagent_host_input::{SubagentHostInputBridge, SubagentHostInputRequest},
    },
    config::Config,
};

/// Bounds each wait on an explicit signal; never the way a test syncs.
/// A loaded Windows runner took 6.4s for a prompt that takes milliseconds
/// (neighbouring tests ran 17s in the same job), so the tripwire sits well
/// clear of that.
const TEST_BUDGET: Duration = Duration::from_secs(60);

/// What the asking tool got back for its question.
#[derive(Debug, PartialEq)]
enum Answered {
    Answer(String),
    Refused(HostInputRefusal),
}

/// A tool whose `ask` call puts one question to the host, and whose
/// `progress` call sends a progress tick per `progress_gate` permit until
/// `finish_gate` lets it end. Both run in parallel.
#[derive(Clone)]
struct HostInputTool {
    answered: Arc<Mutex<Option<Answered>>>,
    progress_gate: Arc<Semaphore>,
    progress_sent: Arc<AtomicUsize>,
    finish_gate: Arc<Semaphore>,
}

impl HostInputTool {
    fn new() -> Self {
        Self {
            answered: Arc::default(),
            progress_gate: Arc::new(Semaphore::new(0)),
            progress_sent: Arc::default(),
            finish_gate: Arc::new(Semaphore::new(0)),
        }
    }

    fn answered(&self) -> Option<Answered> {
        self.answered.lock().unwrap().take()
    }
}

fn question() -> HostInputRequest {
    let question = HostQuestion::new(
        "answer",
        "choose",
        vec![HostChoice::new("yes", "yes")],
        SelectionMode::One,
    )
    .unwrap();
    HostInputRequest::questionnaire("delegated", vec![question]).unwrap()
}

impl Tool for HostInputTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "interact".into(),
            description: "asks the host, or reports progress".into(),
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
                    "headless-view-test",
                    key.clone(),
                ))],
                [],
                ToolMetadata::new(),
                move |context| {
                    Box::pin(async move {
                        if key == "ask" {
                            let answered = match context.request_host_input(question()).await {
                                Ok(response) => {
                                    Answered::Answer(response.answers()["answer"][0].clone())
                                }
                                Err(rho_sdk::Error::HostInputRefused { refusal }) => {
                                    Answered::Refused(refusal)
                                }
                                Err(error) => {
                                    return Err(ToolError::new(
                                        ToolErrorKind::Execution,
                                        error.to_string(),
                                    ))
                                }
                            };
                            *self.answered.lock().unwrap() = Some(answered);
                            return Ok(ToolOutput::text("asked"));
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
                                    if !context.progress().send(ToolProgress::new("tick")).await {
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

/// One model turn calling `interact` with each key, then "done".
fn calling(keys: &[&str]) -> Arc<dyn ModelProvider> {
    let calls = keys
        .iter()
        .map(|key| {
            ContentBlock::ToolCall(ToolCall::new(
                format!("{key}-call"),
                "interact".into(),
                json!({ "key": key }),
            ))
        })
        .collect();
    Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "headless-view"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(calls)),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    ))
}

/// An unattended runtime saved under `root`, whose host offers
/// `questions`, on `provider` with `tool`. Events queue one at a time, so a
/// host that stops reading stalls the run.
async fn unattended_runtime(
    root: &Path,
    config: &Config,
    questions: QuestionSupport,
    provider: Arc<dyn ModelProvider>,
    tool: HostInputTool,
) -> SessionRuntime {
    let runtime = Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .tool(tool)
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    unattended_runtime_on(root, config, questions, runtime, provider).await
}

/// Runs the prompt "go" on `runtime` through `view` and returns its reply.
async fn run_prompt(
    runtime: &mut SessionRuntime,
    view: &mut HeadlessView<'_>,
    config: &Config,
) -> String {
    let prompt = runtime.begin_prompt(config).await.unwrap();
    let input = UserInput::text("go");
    let turn = Turn {
        display: input.clone(),
        model: input,
        calls: Vec::new(),
    };
    match runtime
        .run_prompt_turn(view, &prompt, turn, config)
        .await
        .unwrap()
    {
        TurnOutcome::Completed(outcome) => outcome.text().to_owned(),
        TurnOutcome::Cancelled => panic!("the prompt was cancelled"),
    }
}

/// The parent a delegated child asks through `bridge`.
fn parent_of(bridge: &SubagentHostInputBridge) -> ParentQuestions {
    ParentQuestions {
        bridge: bridge.clone(),
        run_id: "run-1".into(),
        agent_id: "worker".into(),
        parent: SessionId::new(),
    }
}

/// Answers every question the parent receives with "yes".
fn answer_yes(mut requests: tokio::sync::mpsc::Receiver<SubagentHostInputRequest>) {
    tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            let _ = request
                .response
                .send(Ok(HostInputResponse::new().answer("answer", ["yes"])));
        }
    });
}

/// Who the asking session's host routes its question to.
enum Asked {
    /// No one: an unattended session with no parent.
    Nobody,
    /// A parent that answers.
    ListeningParent,
    /// A parent that stopped listening.
    GoneParent,
}

// Covers: a headless session's questions are declined instead of failing
// the run, and a delegated child's go to its parent while one listens.
// Owner: headless host view
#[tokio::test]
async fn questions_go_to_a_listening_parent_or_are_declined() {
    for (asked, expected) in [
        (Asked::Nobody, Answered::Refused(HostInputRefusal::Declined)),
        (Asked::ListeningParent, Answered::Answer("yes".into())),
        (
            Asked::GoneParent,
            Answered::Refused(HostInputRefusal::Declined),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let config = Config::default();
        let tool = HostInputTool::new();
        let bridge = SubagentHostInputBridge::new();
        let (questions, parent) = match asked {
            Asked::Nobody => (QuestionSupport::Unavailable, None),
            Asked::ListeningParent => {
                answer_yes(bridge.bind_parent());
                (QuestionSupport::Available, Some(parent_of(&bridge)))
            }
            Asked::GoneParent => (QuestionSupport::Available, Some(parent_of(&bridge))),
        };
        let mut runtime = unattended_runtime(
            root.path(),
            &config,
            questions,
            calling(&["ask"]),
            tool.clone(),
        )
        .await;
        let mut view = HeadlessView::new().with_parent_questions(parent);
        view.bind(&runtime);

        let reply = timeout(TEST_BUDGET, run_prompt(&mut runtime, &mut view, &config))
            .await
            .expect("the prompt ends");
        assert_eq!(
            (reply, tool.answered()),
            ("done".to_owned(), Some(expected))
        );
        runtime.shutdown().await;
    }
}

// Covers: while a delegated question waits on the parent, the child's other
// events keep flowing, so its parallel tools are not stalled behind it.
// Owner: headless host view
#[tokio::test]
async fn a_question_waiting_on_the_parent_does_not_stall_the_run() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::default();
    let tool = HostInputTool::new();
    let bridge = SubagentHostInputBridge::new();
    let mut requests = bridge.bind_parent();
    let mut runtime = unattended_runtime(
        root.path(),
        &config,
        QuestionSupport::Available,
        calling(&["ask", "progress"]),
        tool.clone(),
    )
    .await;
    let parent = parent_of(&bridge);
    let mut view = HeadlessView::new().with_parent_questions(Some(parent));
    view.bind(&runtime);

    let parent_side = async {
        let request = timeout(TEST_BUDGET, requests.recv())
            .await
            .expect("the question reaches the parent")
            .expect("the bridge stays bound");
        tool.progress_gate.add_permits(2);
        timeout(TEST_BUDGET, async {
            while tool.progress_sent.load(Ordering::Acquire) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("progress flows while the parent has not answered");
        let _ = request
            .response
            .send(Ok(HostInputResponse::new().answer("answer", ["yes"])));
        tool.finish_gate.add_permits(1);
    };
    let (reply, ()) = tokio::join!(
        timeout(TEST_BUDGET, run_prompt(&mut runtime, &mut view, &config)),
        parent_side,
    );

    assert_eq!(
        (reply.expect("the prompt ends"), tool.answered()),
        ("done".to_owned(), Some(Answered::Answer("yes".into())))
    );
    runtime.shutdown().await;
}
