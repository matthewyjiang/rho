use std::str::FromStr;

use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{OperationKind, ToolErrorKind, ToolInvocation},
    Rho, RunEvent, ScopedWorkspacePolicy, SessionOptions, ToolCallId, ToolCompletion, UserInput,
    Workspace,
};

use super::*;

fn call_id() -> ToolCallId {
    ToolCallId::from_str("call-1").unwrap()
}

fn invocation(args: serde_json::Value) -> ToolInvocation {
    ToolInvocation::new(call_id(), args)
}

fn workspace(dir: &TempDir) -> Workspace {
    Workspace::new(dir.path()).unwrap()
}

#[test]
fn coding_tools_register_without_granting_capabilities() {
    let mut registry = ToolRegistry::new();
    register_coding_tools(&mut registry, CodingToolOptions::default()).unwrap();

    let names = registry
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["edit_file", "list_dir", "read_file", "write_file"]);
    assert_eq!(
        CodingToolOptions::new()
            .max_output_bytes(8_000)
            .output_budget(),
        8_000
    );
    assert_eq!(CodingToolOptions::default().output_budget(), 12_000);
}

#[tokio::test]
async fn default_context_denies_read_without_policy() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "secret").unwrap();
    let tool = coding_tools(CodingToolOptions::default())
        .into_iter()
        .find(|tool| tool.spec().name == "read_file")
        .unwrap();
    let (context, _progress) = deny_context(Some(workspace(&dir)));

    let error = tool
        .call(invocation(json!({"path": "note.txt"})), context)
        .await
        .unwrap_err();

    assert_eq!(error.kind(), ToolErrorKind::PolicyDenied);
    assert!(error.message().contains("denied"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),
        "secret"
    );
}

#[tokio::test]
async fn missing_workspace_is_rejected() {
    let tool = coding_tools(CodingToolOptions::default())
        .into_iter()
        .find(|tool| tool.spec().name == "list_dir")
        .unwrap();
    let (context, _progress) = deny_context(None);

    let error = tool
        .call(invocation(json!({"path": "."})), context)
        .await
        .unwrap_err();

    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert!(error.message().contains("workspace is required"));
}

#[tokio::test]
async fn allowed_policy_reads_and_reports_metadata() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello\nworld\n").unwrap();
    let runtime = build_runtime_with_coding_tools(
        ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "model"),
            [
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall {
                        id: "call-1".into(),
                        name: "read_file".into(),
                        arguments: json!({"path": "note.txt", "offset": 1, "limit": 1}),
                    },
                )])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ],
        ),
        workspace(&dir),
        ScopedWorkspacePolicy::new().allow_read_paths(),
        CodingToolOptions::default(),
    );
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start(UserInput::text("read the note"))
        .await
        .unwrap();

    let mut read_output = None;
    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::ToolFinished { result, .. } => match result {
                ToolCompletion::Success(output) => {
                    assert_eq!(output.content(), "hello\n");
                    assert_eq!(
                        output.presentation().operation_kind(),
                        Some(&OperationKind::Read)
                    );
                    assert_eq!(
                        output.presentation().affected_paths(),
                        [std::path::PathBuf::from("note.txt:1-1")]
                    );
                    read_output = Some(output);
                }
                other => panic!("unexpected tool result: {other:?}"),
            },
            RunEvent::Completed { outcome } => {
                assert_eq!(outcome.text(), "done");
                break;
            }
            _ => {}
        }
    }
    assert!(read_output.is_some());
}

#[tokio::test]
async fn allowed_policy_writes_with_diff_metadata_and_progress() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_runtime_with_coding_tools(
        ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "model"),
            [
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall {
                        id: "call-1".into(),
                        name: "write_file".into(),
                        arguments: json!({"path": "nested/out.txt", "content": "created"}),
                    },
                )])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "wrote".into(),
                )])),
            ],
        ),
        workspace(&dir),
        ScopedWorkspacePolicy::new()
            .allow_read_paths()
            .allow_write_paths(),
        CodingToolOptions::default(),
    );
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start(UserInput::text("write a file"))
        .await
        .unwrap();

    let mut saw_progress = false;
    let mut write_metadata = None;
    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::ToolUpdated { progress, .. } => {
                saw_progress = true;
                assert!(progress.text().contains("writing"));
                assert_eq!(
                    progress.presentation().operation_kind(),
                    Some(&OperationKind::Write)
                );
            }
            RunEvent::ToolFinished { result, .. } => match result {
                ToolCompletion::Success(output) => {
                    write_metadata = Some(output.presentation().clone());
                    assert!(output.content().contains("created"));
                    assert!(output.content().contains("+created"));
                }
                other => panic!("unexpected tool result: {other:?}"),
            },
            RunEvent::Completed { .. } => break,
            _ => {}
        }
    }

    assert!(saw_progress);
    let metadata = write_metadata.expect("write metadata");
    assert_eq!(metadata.operation_kind(), Some(&OperationKind::Write));
    assert_eq!(
        metadata.affected_paths(),
        [std::path::PathBuf::from("nested/out.txt")]
    );
    assert!(metadata.unified_diff().unwrap().contains("+created"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/out.txt")).unwrap(),
        "created"
    );
}

#[tokio::test]
async fn default_runtime_policy_keeps_coding_tools_inert() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "safe").unwrap();
    let runtime = build_runtime_with_coding_tools(
        ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "model"),
            [
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall {
                        id: "call-1".into(),
                        name: "read_file".into(),
                        arguments: json!({"path": "note.txt"}),
                    },
                )])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "denied".into(),
                )])),
            ],
        ),
        workspace(&dir),
        ScopedWorkspacePolicy::new(),
        CodingToolOptions::default(),
    );
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let outcome = session.complete("try to read").await.unwrap();
    assert_eq!(outcome.text(), "denied");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),
        "safe"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn hostile_paths_are_rejected_before_file_io() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "unchanged").unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    let tool = coding_tools(CodingToolOptions::default())
        .into_iter()
        .find(|tool| tool.spec().name == "write_file")
        .unwrap();

    for path in [
        "../secret.txt".to_string(),
        outside.path().join("secret.txt").display().to_string(),
        "escape/secret.txt".to_string(),
    ] {
        let (context, _progress) = deny_context(Some(workspace(&root)));
        let error = tool
            .call(
                invocation(json!({"path": path, "content": "overwritten"})),
                context,
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ToolErrorKind::PolicyDenied);
    }
    assert_eq!(
        std::fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
        "unchanged"
    );
}

fn build_runtime_with_coding_tools(
    provider: ScriptedProvider,
    workspace: Workspace,
    policy: ScopedWorkspacePolicy,
    options: CodingToolOptions,
) -> Rho {
    let mut builder = Rho::builder()
        .provider(provider)
        .workspace(workspace)
        .workspace_policy(policy);
    for tool in coding_tools(options) {
        builder = builder.tool_shared(tool);
    }
    builder.build().unwrap()
}
