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
    AgentCatalog::from_sources(AgentCatalogSources {
        rho_home: vec![
            crate::agent::AgentSource::read(PathBuf::from("host-worker.md"), "---\ndescription: host implementation\n---\nUse this child policy".into()),
            crate::agent::AgentSource::read(PathBuf::from("external-worker.md"), "---\ndescription: external implementation\nruntime: claude-cli\ntools: [Read]\n---\nUse the external runtime".into()),
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
        Self::with_capabilities(delegate_result, /*capabilities*/ None)
    }

    /// `capabilities` is T3's optional `orchestrator_capabilities` tool.
    fn with_capabilities(
        delegate_result: Result<ToolOutput, ToolError>,
        capabilities: Option<Result<ToolOutput, ToolError>>,
    ) -> Self {
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
            .chain(capabilities.map(|result| ("orchestrator_capabilities", result)))
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

// Covers: a per-call model always reaches the child's rebind command; T3 is
// queried for a label target only for a `provider/model` reference; and the
// launch still succeeds (with the parent's label) when no safe target applies
// -- the capabilities tool is absent, the call fails, or the model is not
// offered. `asks_t3` and `target` are explicit per row, not derived.
// Owner: host agent launch forwarding, including parsed shared tool arguments.
#[tokio::test]
async fn model_override_reaches_the_child_command_and_an_offered_target() {
    const LUNA: &str = "openai-codex/gpt-6-luna";
    let prompt = "\nInspect the parser\n\n--model is literal task text\n";
    let offers_luna = || {
        Some(Ok(ToolOutput::text("capabilities")
            .with_structured_content(json!({
                "inheritedProviderInstanceId": "rho",
                "inheritedOptions": [{"id": "mode", "value": "plan"}],
                "providers": [{
                    "providerInstanceId": "rho",
                    "models": [{"id": LUNA, "options": [{"id": "mode", "values": ["plan"]}]}],
                }],
            }))))
    };
    let luna_target = json!({"model": LUNA, "options": [{"id": "mode", "value": "plan"}]});
    let failed = || Some(Err(ToolError::new(ToolErrorKind::Execution, "host down")));
    for (case, model, capabilities, asks_t3, target) in [
        (
            "no model override: no query, no target",
            None,
            None,
            false,
            None,
        ),
        (
            "alias: skipped before the query, no target",
            Some("@review"),
            offers_luna(),
            false,
            None,
        ),
        (
            "offered provider/model: query, target with the parent's options",
            Some(LUNA),
            offers_luna(),
            true,
            Some(luna_target),
        ),
        (
            "failed query: launch proceeds with the parent's label",
            Some(LUNA),
            failed(),
            true,
            None,
        ),
        (
            "no capabilities tool: no query, parent's label",
            Some(LUNA),
            None,
            false,
            None,
        ),
    ] {
        let fixture = Fixture::with_capabilities(Ok(started_reply()), capabilities);
        let mut arguments = json!({"agent_id": "host-worker", "prompt": prompt});
        if let Some(model) = model {
            arguments["model"] = json!(model);
        }
        let output = fixture.launch_args(arguments).await.unwrap();
        let task = match model {
            Some(model) => format!("/agent:host-worker --model {model} -- {prompt}"),
            None => format!("/agent:host-worker {prompt}"),
        };
        let mut delegated = json!({"task": task, "mode": "async"});
        if let Some(target) = target {
            delegated["target"] = target;
        }
        let call = |name, arguments| {
            (
                namespaced_tool_name("t3-code", name),
                invocation(json!({})).id().clone(),
                arguments,
            )
        };
        let mut expected = Vec::new();
        if asks_t3 {
            expected.push(call("orchestrator_capabilities", json!({"search": model})));
        }
        expected.push(call("delegate_task", delegated));
        assert_eq!(*fixture.calls.lock().unwrap(), expected, "{case}");
        assert_eq!(
            output.structured_content(),
            Some(
                &serde_json::to_value(
                    AgentRunView::started("task-1".into(), "host-worker".into(),)
                )
                .unwrap()
            ),
            "{case}",
        );
    }
}

// Covers: a cancel during the capabilities query must cancel the launch and
// never send delegate_task, so a cancelled turn cannot spawn an untracked
// child. Owner: host agent launch forwarding.
#[tokio::test]
async fn capabilities_cancellation_cancels_the_launch_before_delegating() {
    let fixture =
        Fixture::with_capabilities(Ok(started_reply()), Some(Err(ToolError::cancelled())));
    let error = fixture
        .launch_args(json!({
            "agent_id": "host-worker",
            "prompt": "Implement the parser",
            "model": "openai-codex/gpt-6-luna",
        }))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::Cancelled);
    assert_eq!(
        *fixture.calls.lock().unwrap(),
        vec![(
            namespaced_tool_name("t3-code", "orchestrator_capabilities"),
            invocation(json!({})).id().clone(),
            json!({"search": "openai-codex/gpt-6-luna"}),
        )]
    );
    assert!(fixture.orchestrator.runs().is_empty());
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
