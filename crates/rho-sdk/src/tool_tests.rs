use std::{future::pending, num::NonZeroUsize, str::FromStr, sync::Arc};

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    model::ToolSpec, ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
    CancellationToken, CapabilityRequest, CapabilitySource, ProcessEnvironment, ProcessExecution,
    ProcessInvocation, ProcessOutputLimits, ScopedWorkspacePolicy, ToolCallId,
};

use super::{
    tool_progress_channel, OperationKind, ScriptedTool, ScriptedToolOutcome, Tool, ToolContext,
    ToolErrorKind, ToolExecutionPolicy, ToolInvocation, ToolMetadata, ToolOutput,
    ToolPreparationContext, ToolRegistry, ToolResource, ToolResourceAccess,
};

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: format!("{name} description"),
        input_schema: json!({"type": "object"}),
    }
}

fn invocation() -> ToolInvocation {
    ToolInvocation::new(ToolCallId::from_str("call-1").unwrap(), json!({"value": 1}))
}

fn context(cancellation: CancellationToken) -> (ToolContext, super::ToolProgressReceiver) {
    let (progress, receiver) = tool_progress_channel(NonZeroUsize::new(4).unwrap());
    (
        ToolContext::new(
            Some(crate::Workspace::new(std::env::temp_dir()).unwrap()),
            cancellation,
            progress,
        ),
        receiver,
    )
}

#[tokio::test]
async fn default_preparation_preserves_the_exclusive_call_path() {
    let metadata = ToolMetadata::new().operation(OperationKind::Read);
    let tool = ScriptedTool::new(
        spec("legacy"),
        ScriptedToolOutcome::Success(ToolOutput::text("called")),
    );
    let cancellation = CancellationToken::new();
    let (context, _progress) = context(cancellation);
    let preparation = ToolPreparationContext::from_execution(&context);
    let prepared = tool.prepare(invocation(), preparation).await.unwrap();

    assert_eq!(prepared.execution_policy(), &ToolExecutionPolicy::Exclusive);
    assert!(prepared.capabilities().is_empty());
    assert_eq!(prepared.start_metadata(), &ToolMetadata::default());
    assert_eq!(prepared.execute(context).await.unwrap().content(), "called");

    // Metadata remains available through the explicit prepared constructor.
    let prepared = super::PreparedToolInvocation::resource_aware(
        [ToolResourceAccess::shared(ToolResource::session_state())],
        [],
        metadata.clone(),
        |_context| Box::pin(async { Ok(ToolOutput::text("prepared")) }),
    );
    assert_eq!(prepared.start_metadata(), &metadata);
}

#[test]
fn resource_debug_output_omits_secret_values() {
    let resource = ToolResource::opaque("private-tool", "secret-key");
    let debug = format!("{resource:?}");

    assert!(debug.contains("Opaque"));
    assert!(!debug.contains("private-tool"));
    assert!(!debug.contains("secret-key"));
}

#[tokio::test]
async fn every_tool_call_receives_cooperative_cancellation() {
    let tool = ScriptedTool::new(spec("wait"), ScriptedToolOutcome::WaitForCancellation);
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let (context, _progress) = context(cancellation);

    let (result, ()) = tokio::join!(tool.call(invocation(), context), async move {
        cancel.cancel();
    });

    assert_eq!(result.unwrap_err().kind(), ToolErrorKind::Cancelled);
}

#[derive(Debug)]
struct PendingApproval;

impl ApprovalHandler for PendingApproval {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(pending::<ApprovalDecision>())
    }
}

#[tokio::test]
async fn cancellation_interrupts_a_pending_host_approval() {
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let (progress, _receiver) = tool_progress_channel(NonZeroUsize::new(1).unwrap());
    let context = ToolContext::with_security(
        None,
        Arc::new(
            ScopedWorkspacePolicy::new()
                .allow_processes()
                .require_process_approval(),
        ),
        Arc::new(PendingApproval),
        Arc::default(),
        Arc::default(),
        cancellation,
        progress,
    );

    let (result, ()) = tokio::join!(
        context.authorize(CapabilityRequest::process(
            ProcessExecution::new(
                "/workspace",
                ProcessInvocation::executable("/usr/bin/cargo", vec!["test".into()]),
                ProcessEnvironment::Empty,
                ProcessOutputLimits::new(1024, None),
            ),
            CapabilitySource::host_tool("test"),
        )),
        async move { cancel.cancel() }
    );

    assert!(matches!(
        result,
        Err(error) if error.kind() == crate::AuthorizationDenialKind::Cancelled
    ));
}

#[test]
fn registry_rejects_duplicate_names_without_replacing_the_first_tool() {
    let first = ScriptedTool::new(
        spec("duplicate"),
        ScriptedToolOutcome::Success(ToolOutput::text("first")),
    );
    let second = ScriptedTool::new(
        spec("duplicate"),
        ScriptedToolOutcome::Success(ToolOutput::text("second")),
    );
    let mut registry = ToolRegistry::new();
    registry.register(first).unwrap();

    let error = registry.register(second).unwrap_err();

    assert_eq!(error.name(), "duplicate");
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>(),
        ["duplicate"]
    );
}

/// Tool that overrides neither `call` nor `prepare`, which is a programming
/// error the SDK must report rather than loop on.
struct UnimplementedTool;

impl Tool for UnimplementedTool {
    fn spec(&self) -> ToolSpec {
        spec("unimplemented")
    }
}

#[tokio::test]
async fn a_tool_implementing_neither_call_nor_prepare_reports_the_mistake() {
    let (context, _receiver) = context(CancellationToken::new());

    let error = UnimplementedTool
        .call(invocation(), context)
        .await
        .expect_err("a tool with no implementation cannot succeed");

    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert!(
        error
            .message()
            .contains("implements neither Tool::call nor Tool::prepare"),
        "unexpected message: {}",
        error.message()
    );
}


