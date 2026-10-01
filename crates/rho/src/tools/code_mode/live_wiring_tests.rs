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
