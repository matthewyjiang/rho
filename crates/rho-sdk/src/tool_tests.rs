use std::{num::NonZeroUsize, path::PathBuf, str::FromStr};

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{model::ToolSpec, CancellationToken, ToolCallId};

use super::{
    tool_progress_channel, OperationKind, ScriptedTool, ScriptedToolOutcome, Tool, ToolContext,
    ToolError, ToolErrorKind, ToolInvocation, ToolMetadata, ToolOutput, ToolProgress, ToolRegistry,
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
        ToolContext::new("/workspace", cancellation, progress),
        receiver,
    )
}

#[tokio::test]
async fn scripted_tool_returns_structured_success_and_progress() {
    let metadata = ToolMetadata::new()
        .operation(OperationKind::Write)
        .affected_path("src/lib.rs")
        .diff("+new line");
    let tool = ScriptedTool::new(
        spec("edit"),
        ScriptedToolOutcome::Success(ToolOutput::text("updated").metadata(metadata.clone())),
    )
    .progress([ToolProgress::message("editing").units(1, 2)]);
    let (context, mut progress) = context(CancellationToken::new());

    let output = tool.call(invocation(), context).await.unwrap();

    assert_eq!(output.content(), "updated");
    assert_eq!(output.presentation(), &metadata);
    assert_eq!(progress.recv().await.unwrap().text(), "editing");
}

#[tokio::test]
async fn scripted_tool_returns_typed_failure_and_invalid_arguments() {
    let tool = ScriptedTool::new(
        spec("parse"),
        ScriptedToolOutcome::Failure(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "missing value",
        )),
    );
    let (context, _progress) = context(CancellationToken::new());

    let error = tool.call(invocation(), context).await.unwrap_err();

    assert_eq!(error.kind(), ToolErrorKind::InvalidArguments);
    assert_eq!(error.message(), "missing value");
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

#[test]
fn metadata_exposes_structured_paths_commands_urls_and_diffs() {
    let metadata = ToolMetadata::new()
        .operation(OperationKind::Execute)
        .affected_path("Cargo.toml")
        .command_summary("cargo test")
        .url("https://example.com")
        .diff("+dependency");

    assert_eq!(metadata.operation_kind(), Some(&OperationKind::Execute));
    assert_eq!(metadata.affected_paths(), [PathBuf::from("Cargo.toml")]);
    assert_eq!(metadata.command_summary_text(), Some("cargo test"));
    assert_eq!(metadata.urls(), ["https://example.com"]);
    assert_eq!(metadata.unified_diff(), Some("+dependency"));
}
