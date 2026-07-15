//! Host questionnaire responses and capability approvals.
//!
//! ```sh
//! cargo run -p rho-sdk --example questionnaire_approval
//! ```

use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput},
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, CapabilityRequest,
    HostChoice, HostInputRequest, HostInputResponse, HostQuestion, Rho, RunEvent,
    ScopedWorkspacePolicy, SelectionMode, SessionOptions, UserInput,
};
use serde_json::json;

#[derive(Debug)]
struct AskModeTool;

impl Tool for AskModeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ask_mode".into(),
            description: "Ask the host which mode to use".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let question = HostQuestion::new(
                "mode",
                "Which mode?",
                vec![
                    HostChoice::new("fast", "Fast"),
                    HostChoice::new("safe", "Safe"),
                ],
                SelectionMode::One,
            )
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            let request = HostInputRequest::questionnaire("choose mode", vec![question])
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            let response = context
                .request_host_input(request)
                .await
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            let mode = response
                .answers()
                .get("mode")
                .and_then(|values| values.first())
                .cloned()
                .unwrap_or_else(|| "unknown".into());
            Ok(ToolOutput::text(mode))
        })
    }
}

#[derive(Debug)]
struct ProcessTool;

impl Tool for ProcessTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "run_process".into(),
            description: "Request process execution through workspace policy".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            context
                .authorize(CapabilityRequest::ExecuteProcess {
                    program: "cargo".into(),
                    arguments: vec!["test".into()],
                })
                .await
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            Ok(ToolOutput::text("process approved"))
        })
    }
}

#[derive(Debug)]
struct AllowOnceApprovals;

impl ApprovalHandler for AllowOnceApprovals {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            println!(
                "approval requested: {} ({})",
                request.reason(),
                match request.capability() {
                    CapabilityRequest::ExecuteProcess { program, .. } => program.as_str(),
                    CapabilityRequest::ReadPath { .. } => "read",
                    CapabilityRequest::WritePath { .. } => "write",
                    CapabilityRequest::NetworkAccess { .. } => "network",
                    _ => "capability",
                }
            );
            ApprovalDecision::AllowOnce
        })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), rho_sdk::Error> {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "q-1".into(),
                    name: "ask_mode".into(),
                    arguments: json!({}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "p-1".into(),
                    name: "run_process".into(),
                    arguments: json!({}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "configured".into(),
            )])),
        ],
    );
    let rho = Rho::builder()
        .provider(provider)
        .tool(AskModeTool)
        .tool(ProcessTool)
        .workspace_policy(
            ScopedWorkspacePolicy::new()
                .allow_processes()
                .require_process_approval(),
        )
        .approval_handler(AllowOnceApprovals)
        .build()?;
    let session = rho.session(SessionOptions::default()).await?;
    let mut run = session.start(UserInput::text("configure and test")).await?;

    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::HostInputRequested { request } => {
                println!("questionnaire: {}", request.title());
                run.respond(
                    request.id().clone(),
                    HostInputResponse::new().answer("mode", ["safe"]),
                )
                .await?;
            }
            RunEvent::ToolFinished { call_id, result } => {
                println!("tool finished: {call_id} -> {result:?}");
            }
            RunEvent::Completed { outcome } => {
                println!("final={}", outcome.text());
            }
            _ => {}
        }
    }

    let outcome = run.outcome().await?;
    assert_eq!(outcome.text(), "configured");
    Ok(())
}
