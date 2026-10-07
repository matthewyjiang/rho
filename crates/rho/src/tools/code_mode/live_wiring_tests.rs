//! Contract coverage through real ToolHosts, not a test-only bridge.

use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::ToolSpec,
    tool::{
        Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput,
        ToolProgress,
    },
    ApprovalAuditDecision, ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
    CapabilityRequest, CapabilitySource, PathScope, PolicyDecision, ToolHost, ToolHostCall,
    WorkspacePolicy,
};
use serde_json::{json, Value};

use super::{exposure::CodeModeSurface, tool::CodeModeTool, CODEMODE_TOOL_NAME, TOOL_SEARCH_NAME};

/// One configurable sibling for value conversion, authorization, and relay tests.
struct StubTool {
    name: &'static str,
    outcome: Result<ToolOutput, ToolError>,
    authorize: bool,
    progress: bool,
    ask: bool,
    calls: Option<Arc<Mutex<usize>>>,
}

impl StubTool {
    fn output(output: ToolOutput) -> Self {
        Self {
            name: "probe",
            outcome: Ok(output),
            authorize: false,
            progress: false,
            ask: false,
            calls: None,
        }
    }
}

impl Tool for StubTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "fixture sibling".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn output_schema(&self) -> Option<Value> {
        self.outcome
            .as_ref()
            .ok()?
            .structured_content()
            .map(|_| json!({"$defs": {"Payload": {"type": "object"}}, "$ref": "#/$defs/Payload"}))
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            if let Some(calls) = &self.calls {
                *calls.lock().unwrap() += 1;
            }
            if self.authorize {
                context
                    .authorize(CapabilityRequest::read_path(
                        "/work/input",
                        PathScope::PrimaryWorkspace,
                        CapabilitySource::host_tool(self.name),
                    ))
                    .await
                    .map_err(|error| ToolError::policy_denied(&error))?;
            }
            if self.progress {
                context
                    .progress()
                    .send(ToolProgress::message("halfway"))
                    .await;
            }
            if self.ask {
                let question = rho_sdk::HostQuestion::new(
                    "answer",
                    "choose",
                    vec![rho_sdk::HostChoice::new("yes", "yes")],
                    rho_sdk::SelectionMode::One,
                )
                .unwrap();
                let request =
                    rho_sdk::HostInputRequest::questionnaire("nested", vec![question]).unwrap();
                let response = context
                    .request_host_input(request)
                    .await
                    .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
                return Ok(ToolOutput::text(response.answers()["answer"][0].clone()));
            }
            self.outcome.clone()
        })
    }
}

pub(super) fn surface(tools: Vec<Arc<dyn Tool>>) -> Arc<CodeModeSurface> {
    let surface = Arc::new(CodeModeSurface::default());
    surface.sync(&tools);
    surface
}

fn host(probe: StubTool) -> ToolHost {
    ToolHost::builder()
        .event_capacity(crate::app::sdk_config::parallel_tool_limit())
        .tool(CodeModeTool::new(surface(vec![Arc::new(probe)])))
        .build()
        .unwrap()
}

/// The script's `result`, or its failure text. A raising script is a
/// completed failure that keeps partial output, not a host error.
pub(super) async fn script(host: &ToolHost, source: &str) -> Result<Value, String> {
    let output = host
        .invoke(ToolHostCall::new(
            CODEMODE_TOOL_NAME,
            json!({"script": source}),
        ))
        .await
        .map_err(|error| error.to_string())?;
    if output.is_failure() {
        return Err(output.content().to_owned());
    }
    Ok(output.structured_content().unwrap()["return_value"].clone())
}

// Covers: completed failures stay values, host status wins over payload status,
// and floats remain numeric through JSON -> Starlark -> JSON.
// Owner: codemode engine / ToolHost bridge.
#[tokio::test]
async fn nested_outcomes_preserve_values_and_failure_status() {
    let cases = [
        (
            Ok(ToolOutput::text("text")),
            Ok(json!({"content": "text", "data": null, "is_error": false})),
        ),
        (
            Ok(ToolOutput::text("text").with_structured_content(json!({"value": 1.5}))),
            Ok(json!({"content": "text", "data": {"value": 1.5}, "is_error": false})),
        ),
        (
            Ok(ToolOutput::text("failed")
                .with_structured_content(json!({"is_error": false, "exit_code": 3}))
                .failed()),
            Ok(
                json!({"content": "failed", "data": {"is_error": false, "exit_code": 3}, "is_error": true}),
            ),
        ),
        (
            Ok(ToolOutput::text("failed").failed()),
            Ok(json!({"content": "failed", "data": null, "is_error": true})),
        ),
        (
            Ok(ToolOutput::text("failed")
                .with_structured_content(json!([3]))
                .failed()),
            Ok(json!({"content": "failed", "data": [3], "is_error": true})),
        ),
        (
            Err(ToolError::new(ToolErrorKind::Execution, "boom")),
            Err(()),
        ),
        (
            Err(ToolError::new(ToolErrorKind::PolicyDenied, "plan mode")),
            Err(()),
        ),
    ];
    for (outcome, expected) in cases {
        let mut probe = StubTool::output(ToolOutput::text(""));
        probe.outcome = outcome;
        let surface = surface(vec![Arc::new(probe)]);
        let schema = surface.search("probe", 1).remove(0).returns;
        let host = ToolHost::builder()
            .tool(CodeModeTool::new(surface))
            .build()
            .unwrap();
        let actual = script(&host, "result = call_tool(\"probe\")")
            .await
            .map_err(|_| ());
        if let Ok(value) = &actual {
            jsonschema::validator_for(&schema)
                .unwrap()
                .validate(value)
                .unwrap();
        }
        assert_eq!(actual, expected);
    }
    assert_eq!(
        script(
            &host(StubTool::output(
                ToolOutput::text("").with_structured_content(json!({"value": 1.5}))
            )),
            "result = call_tool(\"probe\")[\"data\"][\"value\"] * 2"
        )
        .await
        .unwrap(),
        json!(3.0)
    );
}

#[derive(Clone)]
struct DiscoveryDefinition {
    spec: ToolSpec,
    output_schema: Option<Value>,
}

/// Models an MCP handle whose definition changes without an inventory update.
struct DiscoveryTool {
    definition: Mutex<DiscoveryDefinition>,
}

impl Tool for DiscoveryTool {
    fn spec(&self) -> ToolSpec {
        self.definition.lock().unwrap().spec.clone()
    }

    fn output_schema(&self) -> Option<Value> {
        self.definition.lock().unwrap().output_schema.clone()
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let title = invocation.arguments()["title"].as_str().ok_or_else(|| {
                ToolError::new(ToolErrorKind::InvalidArguments, "title must be a string")
            })?;
            Ok(ToolOutput::text(title))
        })
    }
}

// Covers: discovery includes complete argument contracts, and execution shares
// its inventory; orchestration is not a sibling and recursive calls fail.
// Owner: codemode discovery / ToolHost bridge.
#[tokio::test]
async fn discovery_matches_callable_siblings() {
    let surface = Arc::new(CodeModeSurface::default());
    let orchestration = surface.orchestration_tools();
    // Publishing the complete app inventory must not introduce an Arc cycle.
    let parameters = json!({
        "type": "object",
        "properties": {"title": {"type": "string", "minLength": 1}},
        "required": ["title"],
        "additionalProperties": false,
    });
    let mut tools: Vec<Arc<dyn Tool>> = vec![Arc::new(DiscoveryTool {
        definition: Mutex::new(DiscoveryDefinition {
            spec: ToolSpec {
                name: "mcp__github__create_issue".into(),
                description: "fixture sibling".into(),
                input_schema: parameters.clone(),
            },
            output_schema: None,
        }),
    })];
    tools.extend(orchestration.clone());
    surface.sync(&tools);
    let host = tools
        .into_iter()
        .fold(ToolHost::builder(), |builder, tool| {
            builder.tool_shared(tool)
        })
        .build()
        .unwrap();
    let listed = script(&host, "result = list_tools()").await.unwrap();
    let searched = script(&host, "result = search_tools(\"github\")")
        .await
        .unwrap();
    let discovered = host
        .invoke(ToolHostCall::new(
            TOOL_SEARCH_NAME,
            json!({"query": "github"}),
        ))
        .await
        .unwrap();
    // Script discovery returns compact rows; tool_search keeps full entries.
    assert_eq!(
        (listed, searched.clone()),
        (
            json!([{"name": "mcp__github__create_issue", "description": "fixture sibling"}]),
            json!([{"name": "mcp__github__create_issue", "description": "fixture sibling"}]),
        )
    );
    let expected = json!([{
        "name": "mcp__github__create_issue",
        "description": "fixture sibling",
        "parameters": parameters,
        "returns": super::script_output::schema(/*data_schema*/ None),
    }]);
    assert_eq!(discovered.structured_content(), Some(&expected));
    // A miss keeps empty data but must not read as a bare `[]` to the model,
    // which took that as "MCP server not connected" and gave up.
    let missed = host
        .invoke(ToolHostCall::new(
            TOOL_SEARCH_NAME,
            json!({"query": "memorywhale"}),
        ))
        .await
        .unwrap();
    assert_eq!(missed.structured_content(), Some(&json!([])));
    assert_ne!(missed.content(), "[]");
    assert_eq!(
        script(
            &host,
            "result = [describe_tool(\"mcp__github__create_issue\")]"
        )
        .await
        .unwrap(),
        expected
    );
    let discovery_schema = orchestration[1].output_schema().unwrap();
    jsonschema::validator_for(&discovery_schema)
        .unwrap()
        .validate(discovered.structured_content().unwrap())
        .unwrap();
    assert_eq!(
        script(
            &host,
            "hits = search_tools(\"github\")\nresult = call_tool(hits[0][\"name\"], {\"title\": \"codemode issue\"})[\"content\"]"
        )
        .await
        .unwrap(),
        json!("codemode issue")
    );
    let (progress, _receiver) = rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::MIN);
    let context = ToolContext::new(
        /*workspace*/ None,
        rho_sdk::CancellationToken::new(),
        progress,
    );
    let (nested, _) = surface.snapshot(&context).unwrap();
    assert!(matches!(
        nested
            .invoke(ToolHostCall::new(CODEMODE_TOOL_NAME, json!({})))
            .await,
        Err(rho_sdk::Error::InvalidConfiguration { .. })
    ));
}

// Covers: new searches and bridges refresh live contracts without sync,
// while an existing bridge retains its original catalog.
// Owner: codemode live discovery and bridge snapshots (runtime contract).
#[test]
fn discovery_refreshes_live_definitions_without_resync() {
    use super::{bridge::ToolHostBridge, exposure::ToolCatalogEntry};

    let tool = Arc::new(DiscoveryTool {
        definition: Mutex::new(DiscoveryDefinition {
            spec: ToolSpec {
                name: "mcp__github__create_issue".into(),
                description: "original definition".into(),
                input_schema: json!({"type": "object"}),
            },
            output_schema: Some(json!({"type": "string"})),
        }),
    });
    let surface = surface(vec![tool.clone()]);
    let (progress, _receiver) = rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::MIN);
    let context = ToolContext::new(
        /*workspace*/ None,
        rho_sdk::CancellationToken::new(),
        progress,
    );
    let frozen = ToolHostBridge::new(surface.clone(), context.clone()).unwrap();
    let original = frozen.describe("mcp__github__create_issue").unwrap();
    let revised = DiscoveryDefinition {
        spec: ToolSpec {
            name: "mcp__github__create_issue".into(),
            description: "revised definition".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"priority": {"type": "integer"}},
                "required": ["priority"],
            }),
        },
        output_schema: Some(json!({"type": "object"})),
    };
    *tool.definition.lock().unwrap() = revised.clone();
    let expected = ToolCatalogEntry {
        name: revised.spec.name,
        description: revised.spec.description,
        parameters: revised.spec.input_schema,
        returns: super::script_output::schema(revised.output_schema),
    };
    assert_eq!(
        surface.search(&expected.description, 1),
        vec![expected.clone()]
    );
    let fresh = ToolHostBridge::new(surface, context).unwrap();
    assert_eq!(fresh.describe(&expected.name), Some(expected));
    assert_eq!(frozen.describe(&original.name), Some(original));
}

// Covers: publishing siblings mid-script cannot advertise names its child host rejects.
// Owner: codemode bridge snapshot (runtime contract).
#[tokio::test]
async fn bridge_discovery_and_execution_keep_one_snapshot() {
    let surface = surface(vec![Arc::new(StubTool::output(ToolOutput::text(
        "original",
    )))]);
    let (progress, receiver) = rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::MIN);
    drop(receiver);
    let context = ToolContext::new(
        /*workspace*/ None,
        rho_sdk::CancellationToken::new(),
        progress,
    );
    let bridge = super::bridge::ToolHostBridge::new(surface.clone(), context).unwrap();
    surface.sync(&[Arc::new(StubTool {
        name: "late",
        ..StubTool::output(ToolOutput::text("late"))
    })]);
    assert_eq!(
        bridge
            .search("", usize::MAX)
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["probe"]
    );
    assert_eq!(
        bridge
            .call_tool("probe", json!({}))
            .await
            .unwrap()
            .content(),
        "original"
    );
    assert!(matches!(
        bridge.call_tool("late", json!({})).await,
        Err(super::bridge::BridgeError::Host(
            rho_sdk::Error::InvalidConfiguration { .. }
        ))
    ));
}

// Covers: a cancelled script cannot start another nested tool run.
// Owner: codemode ToolHost bridge.
#[tokio::test]
async fn cancelled_parent_does_not_start_subsequent_nested_calls() {
    use super::bridge::{BridgeError, ToolHostBridge};

    let calls = Arc::new(Mutex::new(0));
    let surface = surface(vec![Arc::new(StubTool {
        calls: Some(calls.clone()),
        ..StubTool::output(ToolOutput::text("ok"))
    })]);
    let (progress, receiver) = rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::MIN);
    drop(receiver);
    let cancellation = rho_sdk::CancellationToken::new();
    let context = ToolContext::new(/*workspace*/ None, cancellation.clone(), progress);
    let bridge = ToolHostBridge::new(surface, context).unwrap();
    bridge.call_tool("probe", json!({})).await.unwrap();

    cancellation.cancel();
    assert!(matches!(
        bridge.call_tool("probe", json!({})).await,
        Err(BridgeError::Cancelled)
    ));
    assert_eq!((*calls.lock().unwrap(), bridge.call_count()), (1, 1));
}

// Covers: scripts branch and loop over actual nested outputs.
// Owner: Starlark evaluator / ToolHost integration.
#[tokio::test]
async fn control_flow_composes_nested_results() {
    let host = host(StubTool::output(ToolOutput::text("ok")));
    assert_eq!(script(&host, "seen = []\nfor name in [\"a\", \"b\", \"c\"]:\n    if name != \"b\":\n        seen.append(call_tool(\"probe\")[\"content\"])\nresult = seen").await.unwrap(), json!(["ok", "ok"]));
}

// Covers: evaluation budget, non-JSON result, and nested-call budget fail at
// their owning seam, without collapsing all causes into a copy assertion.
// Owner: Starlark evaluator and its typed native bridge errors.
#[tokio::test]
async fn invalid_scripts_keep_distinct_failure_causes() {
    use super::bridge::{BridgeError, ToolHostBridge};
    use super::engine::{evaluate_code_mode, EngineLimits};
    use starlark::ErrorKind;
    #[derive(Debug)]
    enum Cause {
        EvaluationBudget,
        NonJsonResult,
        NestedBudget,
    }
    for (source, cause, calls) in [
        (
            "total = 0\nfor i in range(10000000):\n    total += i\n",
            Cause::EvaluationBudget,
            0,
        ),
        ("def f():\n    pass\nresult = f", Cause::NonJsonResult, 0),
        (
            "for _ in range(65):\n    call_tool(\"probe\")",
            Cause::NestedBudget,
            65,
        ),
    ] {
        let surface = surface(vec![Arc::new(StubTool::output(ToolOutput::text("ok")))]);
        let (progress, receiver) =
            rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::MIN);
        drop(receiver);
        let bridge = Arc::new(
            ToolHostBridge::new(
                surface,
                ToolContext::new(
                    /*workspace*/ None,
                    rho_sdk::CancellationToken::new(),
                    progress,
                ),
            )
            .unwrap(),
        );
        let evaluator_bridge = bridge.clone();
        let error = tokio::task::spawn_blocking(move || {
            evaluate_code_mode(source, evaluator_bridge, EngineLimits::default())
        })
        .await
        .unwrap()
        .result
        .unwrap_err();
        match (cause, error.kind()) {
            (Cause::EvaluationBudget, ErrorKind::Other(_))
            | (Cause::NonJsonResult, ErrorKind::Value(_)) => {}
            (Cause::NestedBudget, ErrorKind::Native(error)) => assert!(matches!(
                error.downcast_ref::<BridgeError>(),
                Some(BridgeError::CallLimit {
                    max: 64,
                    requested: 65
                })
            )),
            other => panic!("unexpected cause: {other:?}"),
        }
        assert_eq!(bridge.call_count(), calls);
    }
}

struct ApprovalCounter(Arc<Mutex<usize>>);
impl ApprovalHandler for ApprovalCounter {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        *self.0.lock().unwrap() += 1;
        Box::pin(std::future::ready(ApprovalDecision::AllowForSession))
    }
}
struct RequireApproval;
impl WorkspacePolicy for RequireApproval {
    fn evaluate(&self, _request: &CapabilityRequest) -> PolicyDecision {
        PolicyDecision::RequireApproval {
            reason: "fixture approval".into(),
        }
    }
}

// Covers: child host reuses exact-request approval memory and parent audit.
#[tokio::test]
async fn nested_call_reuses_parent_session_approval() {
    let count = Arc::new(Mutex::new(0));
    let probe: Arc<dyn Tool> = Arc::new(StubTool {
        authorize: true,
        ..StubTool::output(ToolOutput::text("gated-ok"))
    });
    let host = ToolHost::builder()
        .tool_shared(probe.clone())
        .tool(CodeModeTool::new(surface(vec![probe])))
        .workspace_policy(RequireApproval)
        .approval_handler(ApprovalCounter(count.clone()))
        .build()
        .unwrap();
    host.invoke(ToolHostCall::new("probe", json!({})))
        .await
        .unwrap();
    assert_eq!(
        script(&host, "result = call_tool(\"probe\")[\"content\"]")
            .await
            .unwrap(),
        json!("gated-ok")
    );
    assert_eq!(*count.lock().unwrap(), 1);
    assert_eq!(
        host.approval_audit()
            .iter()
            .map(|record| record.decision())
            .collect::<Vec<_>>(),
        vec![
            ApprovalAuditDecision::AllowedForSession,
            ApprovalAuditDecision::AllowedByRememberedApproval
        ]
    );
}

// Covers: a real session drains more progress than its bounded channel holds,
// relays nested questions, and delivers the final restatement before ToolFinished.
// Owner: codemode script thread + bridge through SDK orchestration.
#[tokio::test]
async fn nested_events_reach_parent_and_finish() {
    use rho_sdk::{
        model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall},
        provider::{ScriptedProvider, ScriptedTurn},
        Rho, RunEvent, SessionOptions, UserInput,
    };
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "script".into(),
                    name: CODEMODE_TOOL_NAME.into(),
                    arguments: json!({"script": "for _ in range(12):\n    answer = call_tool(\"probe\")\nresult = answer"}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider.clone())
        .max_parallel_tools(crate::app::sdk_config::parallel_tool_limit())
        .tool(CodeModeTool::new(surface(vec![Arc::new(StubTool {
            progress: true,
            ask: true,
            ..StubTool::output(ToolOutput::text(""))
        })])))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("ask")).await.unwrap();
    let mut questions = 0;
    let mut last = None;
    let mut finished = false;
    // Preserve the former session-relay test's 30-second deadlock bound; all
    // synchronization is on actual events, never a timer or sleep.
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(event) = run.next_event().await {
            match event {
                RunEvent::ToolUpdated { progress, .. } => last = Some(progress.text().to_owned()),
                RunEvent::ToolHostInputRequested { call_id, request } => {
                    assert_eq!(call_id.to_string(), "script");
                    questions += 1;
                    run.respond(
                        request.id().clone(),
                        rho_sdk::HostInputResponse::new().answer("answer", ["yes"]),
                    )
                    .await
                    .unwrap();
                }
                RunEvent::ToolFinished { .. } => {
                    // Rows carry durations; compare the marker and name only.
                    let rows: Vec<String> = last
                        .as_deref()
                        .unwrap_or_default()
                        .lines()
                        .map(|row| row.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
                        .collect();
                    assert_eq!(rows, vec!["✓ probe"; 12]);
                    finished = true;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("nested event relay did not finish");
    assert_eq!((questions, finished), (12, true));
    assert_eq!(run.outcome().await.unwrap().text(), "done");
    let requests = provider.recorded_requests();
    let result = requests[1]
        .messages
        .iter()
        .find_map(|message| match message {
            Message::ToolResult(result) if result.id == "script" => Some(result),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (
            result.ok,
            serde_json::from_str::<Value>(&result.content).unwrap()
        ),
        (
            true,
            json!({"content": "yes", "data": null, "is_error": false})
        )
    );
}

/// The first call can finish only after a queued call starts. Initial siblings
/// rendezvous and finish, freeing slots while the first call remains blocked.
struct RendezvousTool {
    barrier: tokio::sync::Barrier,
    release_first: tokio::sync::Notify,
    width: usize,
}

impl Tool for RendezvousTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "meet".into(),
            description: "fixture sibling".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(self.meet(invocation))
    }

    // The default `prepare` is exclusive, and the child host serializes
    // exclusive calls. Declaring no resources opts into overlap, as MCP tools do.
    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: rho_sdk::tool::ToolPreparationContext,
    ) -> rho_sdk::tool::ToolPrepareFuture<'a> {
        Box::pin(async move {
            Ok(rho_sdk::tool::PreparedToolInvocation::resource_aware(
                [],
                [],
                rho_sdk::tool::ToolMetadata::new(),
                move |_context| Box::pin(self.meet(invocation)),
            ))
        })
    }
}

impl RendezvousTool {
    async fn meet(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
        let id = invocation.arguments()["id"].as_u64().unwrap() as usize;
        if id < self.width {
            self.barrier.wait().await;
        }
        if id == 0 {
            self.release_first.notified().await;
        } else if id == self.width {
            self.release_first.notify_one();
        }
        Ok(ToolOutput::text(id.to_string()))
    }
}

// Covers: queued calls start while the first call is blocked (ordered buffering
// deadlocks here), input order survives out-of-order completion, and an unknown
// tool becomes an is_error value without losing siblings or call counts.
// Owner: codemode batch bridge.
#[tokio::test]
async fn call_tools_runs_batch_concurrently_in_order() {
    let width = crate::app::sdk_config::parallel_tool_limit().get();
    let surface = surface(vec![Arc::new(RendezvousTool {
        barrier: tokio::sync::Barrier::new(width),
        release_first: tokio::sync::Notify::new(),
        width,
    })]);
    let host = ToolHost::builder()
        .tool(CodeModeTool::new(surface))
        .build()
        .unwrap();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        host.invoke(ToolHostCall::new(
            CODEMODE_TOOL_NAME,
            json!({"script": format!("result = [[r[\"content\"], r[\"is_error\"]] for r in call_tools([(\"meet\", {{\"id\": i}}) for i in range({})] + [\"missing\"])]", width + 1)}),
        )),
    )
    .await
    .expect("batched calls did not run concurrently")
    .unwrap();
    let data = output.structured_content().unwrap();
    let returned = data["return_value"].as_array().unwrap();
    // The unknown tool's content is the host's error text; only its flag matters.
    let missing_flag = returned[width + 1][1].clone();
    let expected: Vec<_> = (0..=width)
        .map(|id| json!([id.to_string(), false]))
        .collect();
    assert_eq!(
        (&returned[..=width], missing_flag, data["calls"].clone()),
        (expected.as_slice(), json!(true), json!(width + 2))
    );
}

/// A shell-like sibling: exclusive, with its capability declared in `prepare`
/// so it is authorized before waiting for its execution slot.
struct DeclaredTool;

impl Tool for DeclaredTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "declared".into(),
            description: "fixture sibling".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        unreachable!("the host runs the prepared plan")
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: rho_sdk::tool::ToolPreparationContext,
    ) -> rho_sdk::tool::ToolPrepareFuture<'a> {
        let path = format!("/work/{}", invocation.arguments()["id"]);
        Box::pin(async move {
            Ok(
                rho_sdk::tool::PreparedToolInvocation::exclusive_with_capabilities(
                    [CapabilityRequest::read_path(
                        path,
                        PathScope::PrimaryWorkspace,
                        CapabilitySource::host_tool("declared"),
                    )],
                    rho_sdk::tool::ToolMetadata::new(),
                    |_context| Box::pin(async { Ok(ToolOutput::text("ok")) }),
                ),
            )
        })
    }
}

/// Allows a request only once `barrier` sees every request it waits for
/// pending at once.
struct RendezvousApproval(tokio::sync::Barrier);

impl ApprovalHandler for RendezvousApproval {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            self.0.wait().await;
            ApprovalDecision::AllowOnce
        })
    }

    fn concurrency(&self) -> rho_sdk::ApprovalConcurrency {
        rho_sdk::ApprovalConcurrency::Concurrent
    }
}

// Covers: every call of a batch awaits approval at once, more calls than may
// execute at once, as in a model-issued batch, so a concurrent approver such
// as the Auto classifier is not fed the batch a few calls at a time.
// Owner: codemode batch bridge.
#[tokio::test]
async fn call_tools_awaits_every_approval_at_once() {
    let batch = crate::app::sdk_config::parallel_tool_limit().get() + 2;
    let host = ToolHost::builder()
        .tool(CodeModeTool::new(surface(vec![Arc::new(DeclaredTool)])))
        .workspace_policy(RequireApproval)
        .approval_handler(RendezvousApproval(tokio::sync::Barrier::new(batch)))
        .build()
        .unwrap();
    let source = format!(
        "result = len(call_tools([(\"declared\", {{\"id\": i}}) for i in range({batch})]))"
    );

    // A failure bound only: approvals capped below the batch never meet.
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), script(&host, &source))
        .await
        .expect("every approval was pending at once");

    assert_eq!(result, Ok(json!(batch)));
}

// Covers: the whole batch is checked against the nested-call budget before
// any call starts.
// Owner: codemode batch bridge.
#[tokio::test]
async fn call_tools_budget_rejects_batch_before_starting() {
    use super::bridge::{BridgeError, ToolHostBridge};

    let calls = Arc::new(Mutex::new(0));
    let surface = surface(vec![Arc::new(StubTool {
        calls: Some(calls.clone()),
        ..StubTool::output(ToolOutput::text("ok"))
    })]);
    let (progress, _receiver) = rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::MIN);
    let context = ToolContext::new(
        /*workspace*/ None,
        rho_sdk::CancellationToken::new(),
        progress,
    );
    let bridge = ToolHostBridge::new(surface, context).unwrap();
    let batch = vec![("probe".to_owned(), json!({})); 65];
    assert!(matches!(
        bridge.call_tools(batch).await,
        Err(BridgeError::CallLimit {
            max: 64,
            requested: 65
        })
    ));
    assert_eq!(*calls.lock().unwrap(), 0);
}

// Covers: a failing script keeps its prints and call count and returns a
// completed failure instead of discarding partial work.
// Owner: codemode tool output.
#[tokio::test]
async fn failed_script_keeps_partial_output() {
    let host = host(StubTool::output(ToolOutput::text("ok")));
    let output = host
        .invoke(ToolHostCall::new(
            CODEMODE_TOOL_NAME,
            json!({"script": "call_tool(\"probe\")\nprint(\"before\")\nfail(\"boom\")"}),
        ))
        .await
        .unwrap();
    let data = output.structured_content().unwrap();
    assert_eq!(
        (
            output.is_failure(),
            data["prints"].clone(),
            data["calls"].clone(),
            data["error"].as_str().unwrap().contains("boom"),
        ),
        (true, json!(["before"]), json!(1), true)
    );
}

#[cfg(unix)]
#[path = "workspace_tests.rs"]
mod workspace_tests;
