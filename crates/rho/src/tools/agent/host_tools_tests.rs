use std::{num::NonZeroUsize, sync::Mutex};

use pretty_assertions::assert_eq;
use rho_sdk::{tool::tool_progress_channel, CancellationToken};

use super::*;
use crate::{
    agent::AgentCatalogSources,
    tools::{mcp::exported_name::namespaced_tool_name, sdk_registry::StaticToolBundle},
};

type RecordedCalls = Arc<Mutex<Vec<(String, ToolCallId, Value)>>>;

struct RecordingTool {
    name: String,
    calls: RecordedCalls,
    result: Result<ToolOutput, ToolError>,
}

impl Tool for RecordingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        self.calls.lock().unwrap().push((
            self.name.clone(),
            invocation.id().clone(),
            invocation.into_arguments(),
        ));
        Box::pin(std::future::ready(self.result.clone()))
    }
}

fn invocation(arguments: Value) -> ToolInvocation {
    ToolInvocation::new(ToolCallId::from_string("host-call").unwrap(), arguments)
}

fn context() -> ToolContext {
    let (progress, _receiver) = tool_progress_channel(NonZeroUsize::MIN);
    ToolContext::new(/*workspace*/ None, CancellationToken::new(), progress)
}

fn catalog() -> AgentCatalog {
    AgentCatalog::from_authorized_sources(AgentCatalogSources {
        rho_home: vec![
            (PathBuf::from("host-worker.md"), "---\ndescription: host implementation\n---\nUse this child policy".into()),
            (PathBuf::from("external-worker.md"), "---\ndescription: external implementation\nruntime: claude-cli\ntools: [Read]\n---\nUse the external runtime".into()),
        ],
        ..Default::default()
    }).unwrap()
}

struct Fixture {
    orchestrator: Arc<HostOrchestrator>,
    calls: RecordedCalls,
    catalog: AgentCatalog,
}

impl Fixture {
    fn new(delegate_result: Result<ToolOutput, ToolError>) -> Self {
        let calls = RecordedCalls::default();
        let mut tools = AppToolSet::disabled();
        tools.add_bundle(StaticToolBundle::new(
            [
                ("delegate_task", delegate_result),
                (
                    "task_status",
                    Ok(ToolOutput::text("host progress").with_structured_content(
                        json!({"status": "waiting", "hasPendingChildRuns": true}),
                    )),
                ),
                (
                    "task_cancel",
                    Ok(ToolOutput::text("host stopped").with_structured_content(
                        json!({"taskId": "task-1", "status": "cancel_requested"}),
                    )),
                ),
            ]
            .into_iter()
            .map(|(name, result)| {
                Arc::new(RecordingTool {
                    name: namespaced_tool_name("t3-code", name),
                    calls: calls.clone(),
                    result,
                }) as Arc<dyn Tool>
            })
            .collect(),
        ));
        Self {
            orchestrator: Arc::new(HostOrchestrator::connect(&tools)),
            calls,
            catalog: catalog(),
        }
    }

    fn agent_tool(&self) -> HostAgentTool {
        HostAgentTool::new(
            self.orchestrator.clone(),
            Path::new("."),
            &self.catalog,
            rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
            Arc::new(()),
        )
    }

    async fn launch(&self, agent_id: &str) -> Result<ToolOutput, ToolError> {
        self.launch_args(json!({"agent_id": agent_id, "prompt": "Implement the parser"}))
            .await
    }

    async fn launch_args(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
        let args = parse_agent_args(arguments)?;
        self.agent_tool()
            .launch(
                invocation(json!({})).id().clone(),
                &self.catalog.find(&args.agent_id).unwrap().definition,
                &args,
                context(),
            )
            .await
    }
}

fn started_reply() -> ToolOutput {
    ToolOutput::text("host accepted").with_structured_content(json!({
        "taskId": "task-1", "childThreadId": "thread-1", "status": "running",
    }))
}

// Covers: per-call models must reach the child's rebind, never T3 target.model;
// the host must not claim an effective selection before that child has bound it.
// Owner: host agent launch forwarding, including parsed shared tool arguments.
#[tokio::test]
async fn model_override_is_forwarded_only_in_the_child_command() {
    let prompt = "\nInspect the parser\n\n--model is literal task text\n";
    for model in [None, Some("@review"), Some("openai-codex/gpt-6-luna")] {
        let fixture = Fixture::new(Ok(started_reply()));
        let mut arguments = json!({"agent_id": "host-worker", "prompt": prompt});
        if let Some(model) = model {
            arguments["model"] = json!(model);
        }
        let output = fixture.launch_args(arguments).await.unwrap();
        let task = match model {
            Some(model) => format!("/agent:host-worker --model {model} -- {prompt}"),
            None => format!("/agent:host-worker {prompt}"),
        };
        assert_eq!(
            *fixture.calls.lock().unwrap(),
            vec![(
                namespaced_tool_name("t3-code", "delegate_task"),
                invocation(json!({})).id().clone(),
                json!({"task": task, "mode": "async"}),
            )]
        );
        assert_eq!(
            output.structured_content(),
            Some(
                &serde_json::to_value(
                    AgentRunView::started("task-1".into(), "host-worker".into(),)
                )
                .unwrap()
            )
        );
    }
}

// Covers: launch must keep the host task receipt, fail loud on a bad/missing
// receipt, and never register a phantom run. Owner: host delegation tools.
#[tokio::test]
async fn host_launch_records_only_successful_receipts() {
    let started =
        serde_json::to_value(AgentRunView::started("task-1".into(), "host-worker".into())).unwrap();
    for (reply, expected) in [
        (Ok(started_reply()), Ok(started)),
        (
            Ok(ToolOutput::text("missing task")
                .with_structured_content(json!({"childThreadId": "thread-1"}))),
            Err(ToolErrorKind::Execution),
        ),
        (
            Ok(ToolOutput::text("host rejected").failed()),
            Err(ToolErrorKind::Execution),
        ),
        (
            Err(ToolError::new(ToolErrorKind::PolicyDenied, "host rejected")),
            Err(ToolErrorKind::Execution),
        ),
        (Err(ToolError::cancelled()), Err(ToolErrorKind::Cancelled)),
    ] {
        let fixture = Fixture::new(reply);
        let result = fixture.launch("host-worker").await;
        let successful = result.is_ok();
        assert_eq!(
            result
                .map(|output| output.structured_content().cloned().unwrap())
                .map_err(|error| error.kind()),
            expected
        );
        assert_eq!(fixture.calls.lock().unwrap().len(), 1);
        let runs = fixture.orchestrator.runs();
        let views = runs.iter().map(HostRunView::from).collect::<Vec<_>>();
        assert_eq!(
            serde_json::to_value(views).unwrap(),
            if successful {
                json!([{"task_id": "task-1", "child_thread_id": "thread-1", "agent_id": "host-worker", "title": "Implement the parser"}])
            } else {
                json!([])
            }
        );
    }
}

// Covers: stop must reach T3 with its reason, status must keep T3's opaque
// lifecycle facts, and unsupported messaging must not call MCP. Owner: host tools.
#[tokio::test]
async fn management_forwards_host_operations_without_native_state_mapping() {
    let fixture = Fixture::new(Ok(started_reply()));
    fixture.launch("host-worker").await.unwrap();
    fixture.calls.lock().unwrap().clear();
    let tool = HostAgentsTool {
        orchestrator: fixture.orchestrator.clone(),
        max_output_bytes: rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
    };
    for (arguments, expected, remote) in [
        (
            json!({"action": "status", "id": "task-1"}),
            Ok(
                json!({"action": "status", "task_id": "task-1", "host": {"status": "waiting", "hasPendingChildRuns": true}}),
            ),
            Some(("task_status", json!({"taskId": "task-1"}))),
        ),
        (
            json!({"action": "status", "id": "task-from-reloaded-session"}),
            Ok(
                json!({"action": "status", "task_id": "task-from-reloaded-session", "host": {"status": "waiting", "hasPendingChildRuns": true}}),
            ),
            Some((
                "task_status",
                json!({"taskId": "task-from-reloaded-session"}),
            )),
        ),
        (
            json!({"action": "stop", "id": "task-1", "reason": "task superseded"}),
            Ok(
                json!({"action": "stop", "task_id": "task-1", "host": {"taskId": "task-1", "status": "cancel_requested"}}),
            ),
            Some((
                "task_cancel",
                json!({"taskId": "task-1", "reason": "task superseded"}),
            )),
        ),
        (
            json!({"action": "message", "id": "task-1", "message": "new task"}),
            Err(ToolErrorKind::InvalidArguments),
            None,
        ),
    ] {
        fixture.calls.lock().unwrap().clear();
        let result = tool.call(invocation(arguments), context()).await;
        assert_eq!(
            result
                .map(|output| output.structured_content().cloned().unwrap())
                .map_err(|error| error.kind()),
            expected
        );
        let expected_calls = remote
            .into_iter()
            .map(|(name, args)| {
                (
                    namespaced_tool_name("t3-code", name),
                    invocation(json!({})).id().clone(),
                    args,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(*fixture.calls.lock().unwrap(), expected_calls);
    }
}

// Covers: host mode must not advertise or launch external-runtime definitions.
// Owner: host runtime policy; no environment-dependent catalog discovery needed.
#[tokio::test]
async fn external_runtime_is_not_advertised_or_launched() {
    let fixture = Fixture::new(Ok(started_reply()));
    assert_eq!(
        fixture.agent_tool().spec().input_schema["properties"]["agent_id"]["enum"],
        json!(["explorer", "host-worker", "reviewer", "worker"])
    );
    for model in [None, Some("@review")] {
        let error = fixture
            .launch_args(json!({
                "agent_id": "external-worker", "prompt": "inspect", "model": model,
            }))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ToolErrorKind::InvalidArguments);
    }
    assert_eq!(*fixture.calls.lock().unwrap(), Vec::new());
    assert!(fixture.orchestrator.runs().is_empty());
}

// Covers: under `--t3code`, a session without T3's task tools (such as T3's
// provider status probe) still starts, but launching a child fails loud instead
// of falling back to in-process subagents or recording a phantom run.
// Owner: host delegation tools.
#[tokio::test]
async fn launch_without_host_contract_fails_without_a_run() {
    let fixture = Fixture {
        orchestrator: Arc::new(HostOrchestrator::connect(&AppToolSet::disabled())),
        calls: RecordedCalls::default(),
        catalog: catalog(),
    };
    let error = fixture.launch("host-worker").await.unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert!(fixture.orchestrator.runs().is_empty());
}
