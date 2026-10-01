//! Registry inclusion + nested authorization sharing for live `codemode`.

use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_sdk::model::ToolSpec;
use rho_sdk::tool::{Tool, ToolContext, ToolError, ToolFuture, ToolInvocation, ToolOutput};
use rho_sdk::{
    ApprovalAuditDecision, ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
    CapabilityRequest, CapabilitySource, PathScope, PolicyDecision, ToolHost, ToolHostCall,
    WorkspacePolicy,
};
use serde_json::json;

use super::bridge::CODEMODE_TOOL_NAME;
use super::exposure::ExposureController;
use super::nesting::CodeModeNesting;
use super::tool::CodeModeTool;
use super::tool_search::TOOL_SEARCH_NAME;
use crate::config::Config;
use crate::diagnostics::RuntimeDiagnostics;
use crate::tools::sdk_registry::{AppToolSet, ToolSetOptions};

struct AllowForSessionCounter {
    count: Arc<Mutex<usize>>,
}

impl ApprovalHandler for AllowForSessionCounter {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        *self.count.lock().expect("count") += 1;
        Box::pin(std::future::ready(ApprovalDecision::AllowForSession))
    }
}

struct RequireApprovalPolicy;

impl WorkspacePolicy for RequireApprovalPolicy {
    fn evaluate(&self, _request: &CapabilityRequest) -> PolicyDecision {
        PolicyDecision::RequireApproval {
            reason: "nested test approval".into(),
        }
    }
}

struct GatedTool;

impl Tool for GatedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "host_exec".into(),
            description: "authorize one host operation".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            context
                .authorize(CapabilityRequest::read_path(
                    "/work/input",
                    PathScope::PrimaryWorkspace,
                    CapabilitySource::host_tool("host_exec"),
                ))
                .await
                .map_err(|error| ToolError::policy_denied(&error))?;
            Ok(ToolOutput::text("gated-ok"))
        })
    }
}

// Covers: default app registry must ship codemode + tool_search as model-facing.
// Owner: app tool registry wiring.
#[test]
fn app_tool_set_registers_codemode_and_tool_search() {
    let config = Config::default();
    let tool_set = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    let model_facing: Vec<_> = tool_set
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .filter(|name| name == CODEMODE_TOOL_NAME || name == TOOL_SEARCH_NAME)
        .collect();
    assert_eq!(
        model_facing,
        vec![CODEMODE_TOOL_NAME.to_owned(), TOOL_SEARCH_NAME.to_owned()]
    );
}

// Covers: a nested gated call must reuse the parent run's exact-request approval
// memory (no second prompt) and land in the parent's audit log.
// Owner: codemode nesting (ToolHost::child_builder wiring).
#[tokio::test(flavor = "multi_thread")]
async fn nested_call_reuses_parent_session_approval() {
    let nesting = Arc::new(CodeModeNesting::default());
    let gated: Arc<dyn Tool> = Arc::new(GatedTool);
    nesting.set_tools(std::slice::from_ref(&gated));
    let count = Arc::new(Mutex::new(0));
    let parent = ToolHost::builder()
        .tool_shared(gated)
        .tool(CodeModeTool::new(
            nesting,
            Arc::new(ExposureController::with_default_policy()),
        ))
        .workspace_policy(RequireApprovalPolicy)
        .approval_handler(AllowForSessionCounter {
            count: Arc::clone(&count),
        })
        .build()
        .expect("parent host");

    parent
        .invoke(ToolHostCall::new("host_exec", json!({})))
        .await
        .expect("direct call");
    let nested = parent
        .invoke(ToolHostCall::new(
            CODEMODE_TOOL_NAME,
            json!({ "script": r#"result = call_tool("host_exec", {})["content"]"# }),
        ))
        .await
        .expect("codemode call");

    assert!(
        nested.content().contains("gated-ok"),
        "{}",
        nested.content()
    );
    assert_eq!(*count.lock().expect("count"), 1);
    assert_eq!(
        parent
            .approval_audit()
            .iter()
            .map(|record| record.decision())
            .collect::<Vec<_>>(),
        vec![
            ApprovalAuditDecision::AllowedForSession,
            ApprovalAuditDecision::AllowedByRememberedApproval,
        ]
    );
}

struct ProgressTool;

impl Tool for ProgressTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "slow_read".into(),
            description: "report progress then finish".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            context
                .progress()
                .send(rho_sdk::tool::ToolProgress::message("halfway"))
                .await;
            Ok(ToolOutput::text("read-ok"))
        })
    }
}

// Covers: nested calls must be visible on the parent `codemode` call's progress
// stream (the TUI card), not silent until the script returns.
// Owner: codemode ToolHostBridge event forwarding.
#[tokio::test(flavor = "multi_thread")]
async fn nested_progress_reaches_parent_call() {
    let nesting = Arc::new(CodeModeNesting::default());
    nesting.set_tools(&[Arc::new(ProgressTool) as Arc<dyn Tool>]);
    let parent = ToolHost::builder()
        .tool(CodeModeTool::new(
            nesting,
            Arc::new(ExposureController::with_default_policy()),
        ))
        .build()
        .expect("parent host");
    let mut run = parent
        .start(ToolHostCall::new(
            CODEMODE_TOOL_NAME,
            json!({ "script": r#"result = call_tool("slow_read")["content"]"# }),
        ))
        .expect("start codemode");

    let mut updates = Vec::new();
    while let Some(event) = run.next_event().await {
        if let rho_sdk::ToolHostEvent::Progress(progress) = event {
            updates.push(progress.text().to_owned());
        }
    }

    assert_eq!(
        updates,
        vec![
            "slow_read: running".to_owned(),
            "slow_read: halfway".to_owned(),
            "slow_read: done".to_owned(),
        ]
    );
    assert!(run
        .outcome()
        .await
        .expect("codemode")
        .content()
        .contains("read-ok"));
}

// Covers: nested failures are classified by typed SDK kind, so a policy
// denial reaches the script as `NestedDenied` and other failures stay `Host`.
// Owner: codemode bridge error classification.
#[test]
fn nested_errors_classify_by_kind() {
    use super::bridge::BridgeError;
    use rho_sdk::tool::ToolErrorKind;
    let cases = [
        (
            rho_sdk::Error::Tool(ToolError::new(ToolErrorKind::PolicyDenied, "plan mode")),
            true,
        ),
        (
            rho_sdk::Error::PolicyDenied {
                message: "no".into(),
            },
            true,
        ),
        (
            // Text mentioning "denied" must not be mistaken for a policy denial.
            rho_sdk::Error::Tool(ToolError::new(ToolErrorKind::Execution, "access denied")),
            false,
        ),
    ];
    for (error, denied) in cases {
        let classified = BridgeError::from_nested("host_exec", error);
        assert_eq!(
            matches!(classified, BridgeError::NestedDenied { .. }),
            denied,
            "{classified}"
        );
    }
}

/// Returns one fixed outcome, so a script can observe what `call_tool` resolves to.
struct OutcomeTool(Result<ToolOutput, ToolError>);

impl Tool for OutcomeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "probe".into(),
            description: "returns a fixed outcome".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        let outcome = self.0.clone();
        Box::pin(async move { outcome })
    }
}

/// Runs `script` through a codemode host whose only sibling is `probe`.
async fn run_codemode_with(probe: OutcomeTool, script: &str) -> Result<String, String> {
    let nesting = Arc::new(CodeModeNesting::default());
    nesting.set_tools(&[Arc::new(probe) as Arc<dyn Tool>]);
    let host = ToolHost::builder()
        .tool(CodeModeTool::new(
            nesting,
            Arc::new(ExposureController::with_default_policy()),
        ))
        .build()
        .expect("host");
    host.invoke(ToolHostCall::new(
        CODEMODE_TOOL_NAME,
        json!({ "script": script }),
    ))
    .await
    .map(|output| output.content().to_owned())
    .map_err(|error| error.to_string())
}

// Covers: what a script's `call_tool` resolves to (Pi semantics). Structured
// content wins, on success and on a completed Execution failure (nonzero
// exit); text-only tools resolve to {"content"}; a text-only failure and a
// denial still raise, whatever structured data rides along.
// Owner: codemode bridge + engine result conversion.
#[tokio::test(flavor = "multi_thread")]
async fn call_tool_resolves_structured_content_like_pi() {
    use rho_sdk::tool::ToolErrorKind;
    let payload = json!({"exit_code": 3});
    let script = r#"result = call_tool("probe")"#;
    let cases = [
        (
            "structured success",
            Ok(ToolOutput::text("text").with_structured_content(payload.clone())),
            Ok(json!({"exit_code": 3})),
        ),
        (
            "text-only success",
            Ok(ToolOutput::text("text")),
            Ok(json!({"content": "text"})),
        ),
        (
            "completed failure with structured content",
            Err(ToolError::new(ToolErrorKind::Execution, "exit 3")
                .with_structured_content(payload.clone())),
            Ok(json!({"exit_code": 3})),
        ),
        (
            "text-only failure",
            Err(ToolError::new(ToolErrorKind::Execution, "boom")),
            Err(()),
        ),
        (
            "denial never becomes a value",
            Err(ToolError::new(ToolErrorKind::PolicyDenied, "plan mode")
                .with_structured_content(payload.clone())),
            Err(()),
        ),
    ];
    for (case, outcome, expected) in cases {
        // With no prints, the codemode output is exactly the pretty `result`.
        let observed = run_codemode_with(OutcomeTool(outcome), script)
            .await
            .map(|content| serde_json::from_str::<serde_json::Value>(&content).unwrap())
            .map_err(|_| ());
        assert_eq!(observed, expected, "{case}");
    }
}

// Covers: script discovery reports each tool's result shape: `bash` lists its
// output schema, a text-only tool lists null (the {"content"} fallback).
// Owner: codemode catalog indexing from Tool::output_schema.
#[cfg(unix)]
#[test]
fn catalog_reports_output_schema_as_returns() {
    let config = Config::default();
    let tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    let catalog = tools.exposure().list_script_visible(usize::MAX);
    let required = |name: &str| {
        catalog
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| {
                entry
                    .returns
                    .as_ref()
                    .map(|schema| schema["required"].clone())
            })
    };
    assert_eq!(
        (required("bash"), required("read_file")),
        (
            Some(Some(json!([
                "stdout",
                "stderr",
                "exit_code",
                "truncated",
                "wall_time_ms"
            ]))),
            Some(None)
        )
    );
}
