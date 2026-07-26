use std::{str::FromStr, sync::Arc};

use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{
        OperationKind, ToolErrorKind, ToolExecutionPolicy, ToolInvocation, ToolPreparationContext,
        ToolResource, ToolResourceAccess,
    },
    CancellationToken, CapabilityOperation, PathScope, PolicyDecision, Rho, RunEvent,
    ScopedWorkspacePolicy, SessionOptions, ToolCallId, ToolCompletion, UserInput, Workspace,
    WorkspacePolicy,
};
use std::sync::Mutex;

use crate::sdk_adapter::{coding_tool, deny_context, CodingToolKind, CodingToolOptions};

fn grep_tool(max_output_bytes: usize) -> Arc<dyn rho_sdk::tool::Tool> {
    search_tool(CodingToolKind::Grep, max_output_bytes)
}

fn glob_tool(max_output_bytes: usize) -> Arc<dyn rho_sdk::tool::Tool> {
    search_tool(CodingToolKind::Glob, max_output_bytes)
}

fn search_tool(kind: CodingToolKind, max_output_bytes: usize) -> Arc<dyn rho_sdk::tool::Tool> {
    coding_tool(
        kind,
        CodingToolOptions::new().max_output_bytes(max_output_bytes),
    )
}

fn call_id() -> ToolCallId {
    ToolCallId::from_str("call-1").unwrap()
}

fn invocation(args: serde_json::Value) -> ToolInvocation {
    ToolInvocation::new(call_id(), args)
}

fn workspace(dir: &TempDir) -> Workspace {
    Workspace::new(dir.path()).unwrap()
}

#[derive(Clone)]
struct RecordingPolicy {
    inner: ScopedWorkspacePolicy,
    requests: Arc<Mutex<Vec<rho_sdk::CapabilityRequest>>>,
}

impl RecordingPolicy {
    fn new(inner: ScopedWorkspacePolicy) -> Self {
        Self {
            inner,
            requests: Arc::default(),
        }
    }

    fn requests(&self) -> Vec<rho_sdk::CapabilityRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl WorkspacePolicy for RecordingPolicy {
    fn evaluate(&self, request: &rho_sdk::CapabilityRequest) -> PolicyDecision {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(request.clone());
        self.inner.evaluate(request)
    }
}

#[tokio::test]
async fn grep_prepare_requests_read_on_canonical_root() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
    let tool = grep_tool(12_000);
    let ws = workspace(&dir);

    let prepared = tool
        .prepare(
            invocation(json!({"pattern": "hello", "path": "."})),
            ToolPreparationContext::new(Some(ws.clone()), CancellationToken::new()),
        )
        .await
        .unwrap();

    let capabilities = prepared.capabilities();
    assert_eq!(capabilities.len(), 1);
    let CapabilityOperation::ReadPath { path, scope } = capabilities[0].operation() else {
        panic!("expected ReadPath");
    };
    assert_eq!(path, &dir.path().canonicalize().unwrap());
    assert_eq!(scope, &PathScope::PrimaryWorkspace);

    let ToolExecutionPolicy::ResourceAware { accesses } = prepared.execution_policy() else {
        panic!("expected resource-aware policy");
    };
    assert!(
        accesses.contains(&ToolResourceAccess::shared(ToolResource::directory_tree(
            dir.path().canonicalize().unwrap()
        )))
    );
}

#[tokio::test]
async fn parent_traversal_is_policy_denied() {
    let dir = TempDir::new().unwrap();
    let tool = grep_tool(12_000);
    let result = tool
        .prepare(
            invocation(json!({"pattern": "x", "path": "../outside"})),
            ToolPreparationContext::new(Some(workspace(&dir)), CancellationToken::new()),
        )
        .await;
    let Err(error) = result else {
        panic!("parent traversal must fail prepare");
    };
    assert_eq!(error.kind(), ToolErrorKind::PolicyDenied);
}

#[tokio::test]
async fn missing_workspace_is_rejected() {
    let tool = glob_tool(12_000);
    let (context, _progress) = deny_context(None);
    let error = tool
        .call(invocation(json!({"pattern": "*.rs"})), context)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert!(error.message().contains("workspace is required"));
}

#[tokio::test]
async fn deny_context_blocks_call() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("note.txt"), "secret").unwrap();
    let tool = grep_tool(12_000);
    let (context, _progress) = deny_context(Some(workspace(&dir)));
    let error = tool
        .call(invocation(json!({"pattern": "secret"})), context)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::PolicyDenied);
}

#[tokio::test]
async fn invalid_regex_fails_prepare_without_capability_requests() {
    let dir = TempDir::new().unwrap();
    let tool = grep_tool(12_000);
    let policy = RecordingPolicy::new(ScopedWorkspacePolicy::new().allow_read_paths());

    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: "grep".into(),
                    arguments: json!({"pattern": "("}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .workspace(workspace(&dir))
        .workspace_policy(policy.clone())
        .tool_shared(tool)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("search")).await.unwrap();
    let mut completion = None;
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolFinished { result, .. } = event {
            completion = Some(result);
        }
    }
    run.outcome().await.unwrap();

    let ToolCompletion::Failure(failure) = completion.expect("tool result") else {
        panic!("invalid regex must fail");
    };
    assert_eq!(failure.kind(), ToolErrorKind::Execution);
    assert!(failure.message().contains("invalid pattern"));
    assert!(policy.requests().is_empty());
}

#[tokio::test]
async fn allowed_policy_runs_grep_and_glob_with_read_metadata() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello world\n").unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn main() {}\n").unwrap();

    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: "grep".into(),
                    arguments: json!({"pattern": "hello"}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-2".into(),
                    name: "glob".into(),
                    arguments: json!({"pattern": "*.rs"}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );

    let mut builder = Rho::builder()
        .provider(provider)
        .workspace(workspace(&dir))
        .workspace_policy(ScopedWorkspacePolicy::new().allow_read_paths());
    builder = builder.tool_shared(grep_tool(12_000));
    builder = builder.tool_shared(glob_tool(12_000));
    let runtime = builder.build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("search")).await.unwrap();

    let mut outputs = Vec::new();
    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::ToolFinished { result, .. } => match result {
                ToolCompletion::Success(output) => {
                    assert_eq!(
                        output.presentation().operation_kind(),
                        Some(&OperationKind::Read)
                    );
                    outputs.push(output.content().to_string());
                }
                other => panic!("unexpected tool result: {other:?}"),
            },
            RunEvent::Completed { .. } => break,
            _ => {}
        }
    }

    assert_eq!(outputs.len(), 2);
    assert!(outputs[0].contains("note.txt"), "{}", outputs[0]);
    assert!(outputs[0].contains("hello world"), "{}", outputs[0]);
    assert!(outputs[1].contains("lib.rs"), "{}", outputs[1]);
}
