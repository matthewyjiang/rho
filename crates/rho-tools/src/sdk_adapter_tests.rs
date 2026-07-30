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
    CancellationToken, Rho, RunEvent, ScopedWorkspacePolicy, SessionOptions, ToolCallId,
    ToolCompletion, UserInput, Workspace,
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

async fn prepared_policy(
    tool: &Arc<dyn rho_sdk::tool::Tool>,
    workspace: Workspace,
    arguments: serde_json::Value,
) -> Result<ToolExecutionPolicy, rho_sdk::tool::ToolError> {
    tool.prepare(
        invocation(arguments),
        ToolPreparationContext::new(Some(workspace), CancellationToken::new()),
    )
    .await
    .map(|prepared| prepared.execution_policy().clone())
}

#[tokio::test]
async fn preparation_canonicalizes_relative_absolute_symlink_and_granted_root_reads() {
    let primary = tempfile::tempdir().unwrap();
    let primary_path = primary.path().join("note.txt");
    std::fs::write(&primary_path, "hello").unwrap();
    let tool = coding_tool(CodingToolKind::ReadFile, CodingToolOptions::default());
    let primary_workspace = workspace(&primary);

    let relative = prepared_policy(
        &tool,
        primary_workspace.clone(),
        json!({"path": "note.txt"}),
    )
    .await
    .unwrap();
    let absolute = prepared_policy(
        &tool,
        primary_workspace.clone(),
        json!({"path": primary_path.to_string_lossy()}),
    )
    .await
    .unwrap();
    assert_eq!(relative, absolute);

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&primary_path, primary.path().join("alias.txt")).unwrap();
        let symlink = prepared_policy(&tool, primary_workspace, json!({"path": "alias.txt"}))
            .await
            .unwrap();
        assert_eq!(relative, symlink);
    }

    let granted = tempfile::tempdir().unwrap();
    let granted_path = granted.path().join("shared.txt");
    std::fs::write(&granted_path, "shared").unwrap();
    let granted_workspace = workspace(&primary)
        .with_granted_root(granted.path())
        .unwrap();
    let granted_policy = prepared_policy(
        &tool,
        granted_workspace,
        json!({"path": granted_path.to_string_lossy()}),
    )
    .await
    .unwrap();
    assert!(matches!(
        granted_policy,
        ToolExecutionPolicy::ResourceAware { .. }
    ));
}

#[tokio::test]
async fn preparation_reserves_missing_write_membership_and_rejects_parent_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = workspace(&dir);
    let write = coding_tool(CodingToolKind::WriteFile, CodingToolOptions::default());

    let policy = prepared_policy(
        &write,
        workspace.clone(),
        json!({"path": "out.txt", "content": "hello"}),
    )
    .await
    .unwrap();
    let ToolExecutionPolicy::ResourceAware { accesses } = policy else {
        panic!("write_file must opt in to resource-aware execution");
    };
    assert!(accesses.contains(&ToolResourceAccess::exclusive(
        ToolResource::workspace_path(workspace.root().join("out.txt"))
    )));
    assert!(accesses.contains(&ToolResourceAccess::exclusive(
        ToolResource::directory_membership(workspace.root())
    )));

    let read = coding_tool(CodingToolKind::ReadFile, CodingToolOptions::default());
    let error = prepared_policy(&read, workspace, json!({"path": "../shared.txt"}))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::PolicyDenied);
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
async fn write_only_policy_cannot_diff_existing_file_contents() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("secret.txt"), "old secret").unwrap();
    let runtime = build_runtime_with_coding_tools(
        ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "model"),
            [
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall {
                        id: "call-1".into(),
                        name: "write_file".into(),
                        arguments: json!({"path": "secret.txt", "content": "new secret"}),
                    },
                )])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "blocked".into(),
                )])),
            ],
        ),
        workspace(&dir),
        ScopedWorkspacePolicy::new().allow_write_paths(),
        CodingToolOptions::default(),
    );
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session
        .start(UserInput::text("overwrite a file"))
        .await
        .unwrap();

    let mut completion = None;
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolFinished { result, .. } = event {
            completion = Some(result);
        }
    }
    run.outcome().await.unwrap();

    let ToolCompletion::Failure(failure) = completion.expect("write tool result") else {
        panic!("write-only policy must deny existing-file diffs");
    };
    assert_eq!(failure.kind(), ToolErrorKind::PolicyDenied);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("secret.txt")).unwrap(),
        "old secret"
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

#[tokio::test]
async fn apply_patch_prepare_reserves_add_update_and_move_paths() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("old.txt"), "old\n").unwrap();
    let workspace = workspace(&dir);
    let tool = coding_tool(CodingToolKind::ApplyPatch, CodingToolOptions::default());
    let input = "\
*** Begin Patch
*** Add File: nested/new.txt
+created
*** Update File: old.txt
*** Move to: moved.txt
@@
-old
+new
*** End Patch";

    let prepared = tool
        .prepare(
            invocation(json!({"input": input})),
            ToolPreparationContext::new(Some(workspace.clone()), CancellationToken::new()),
        )
        .await
        .unwrap();

    let ToolExecutionPolicy::ResourceAware { accesses } = prepared.execution_policy().clone()
    else {
        panic!("apply_patch must opt in to resource-aware execution");
    };

    let expected_paths = [
        workspace.root().join("nested/new.txt"),
        workspace.root().join("old.txt"),
        workspace.root().join("moved.txt"),
    ];
    for path in &expected_paths {
        assert!(
            accesses.contains(&ToolResourceAccess::exclusive(
                ToolResource::workspace_path(path)
            )),
            "missing exclusive access for {}",
            path.display()
        );
    }
    assert!(accesses.contains(&ToolResourceAccess::exclusive(
        ToolResource::directory_membership(workspace.root().join("nested"))
    )));

    let capability_paths = prepared
        .capabilities()
        .iter()
        .filter_map(|capability| match capability.operation() {
            rho_sdk::CapabilityOperation::ReadPath { path, .. }
            | rho_sdk::CapabilityOperation::WritePath { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    for path in &expected_paths {
        assert!(
            capability_paths.contains(path),
            "missing capability for {}",
            path.display()
        );
    }
}

#[tokio::test]
async fn apply_patch_prepare_rejects_invalid_patch_before_io() {
    let dir = tempfile::tempdir().unwrap();
    let tool = coding_tool(CodingToolKind::ApplyPatch, CodingToolOptions::default());
    let error = match tool
        .prepare(
            invocation(json!({"input": "not a patch"})),
            ToolPreparationContext::new(Some(workspace(&dir)), CancellationToken::new()),
        )
        .await
    {
        Ok(_) => panic!("invalid patch must fail prepare"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), ToolErrorKind::InvalidArguments);
}

// Covers: edit_file prepare must lock an existing target for read+write
// Owner: SDK contract
#[tokio::test]
async fn edit_file_prepare_reserves_existing_target() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sample.txt"), "old").unwrap();
    let workspace = workspace(&dir);
    let tool = coding_tool(CodingToolKind::EditFile, CodingToolOptions::default());

    let prepared = tool
        .prepare(
            invocation(json!({
                "path": "sample.txt",
                "old_string": "old",
                "new_string": "new"
            })),
            ToolPreparationContext::new(Some(workspace.clone()), CancellationToken::new()),
        )
        .await
        .unwrap();

    let ToolExecutionPolicy::ResourceAware { accesses } = prepared.execution_policy().clone()
    else {
        panic!("edit_file must opt in to resource-aware execution");
    };
    assert_eq!(
        accesses,
        vec![ToolResourceAccess::exclusive(ToolResource::workspace_path(
            workspace.root().join("sample.txt")
        ))]
    );

    let capability_paths = prepared
        .capabilities()
        .iter()
        .filter_map(|capability| match capability.operation() {
            rho_sdk::CapabilityOperation::ReadPath { path, .. }
            | rho_sdk::CapabilityOperation::WritePath { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert!(capability_paths.contains(&workspace.root().join("sample.txt")));
    assert_eq!(prepared.capabilities().len(), 2);
}

// Covers: edit_file prepare must reject a missing target before execution
// Owner: SDK contract
#[tokio::test]
async fn edit_file_prepare_rejects_missing_target() {
    let dir = tempfile::tempdir().unwrap();
    let tool = coding_tool(CodingToolKind::EditFile, CodingToolOptions::default());
    let error = match tool
        .prepare(
            invocation(json!({
                "path": "missing.txt",
                "old_string": "old",
                "new_string": "new"
            })),
            ToolPreparationContext::new(Some(workspace(&dir)), CancellationToken::new()),
        )
        .await
    {
        Ok(_) => panic!("missing edit target must fail prepare"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), ToolErrorKind::Execution);
}

// Covers: edit_file prepare must reject empty old_string before path I/O
// Owner: SDK contract
#[tokio::test]
async fn edit_file_prepare_rejects_invalid_args() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sample.txt"), "old").unwrap();
    let tool = coding_tool(CodingToolKind::EditFile, CodingToolOptions::default());
    let error = match tool
        .prepare(
            invocation(json!({
                "path": "sample.txt",
                "old_string": "",
                "new_string": "new"
            })),
            ToolPreparationContext::new(Some(workspace(&dir)), CancellationToken::new()),
        )
        .await
    {
        Ok(_) => panic!("empty old_string must fail prepare"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), ToolErrorKind::InvalidArguments);
    assert!(error.to_string().contains("old_string must not be empty"));
}

// Covers: edit_file success path emits diff metadata and progress
// Owner: SDK contract
#[tokio::test]
async fn allowed_policy_edits_with_diff_metadata_and_progress() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sample.txt"), "alpha beta gamma").unwrap();
    let runtime = build_runtime_with_coding_tools(
        ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "model"),
            [
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall {
                        id: "call-1".into(),
                        name: "edit_file".into(),
                        arguments: json!({
                            "path": "sample.txt",
                            "old_string": "beta",
                            "new_string": "delta"
                        }),
                    },
                )])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "edited".into(),
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
        .start(UserInput::text("edit the sample"))
        .await
        .unwrap();

    let mut saw_progress = false;
    let mut edit_metadata = None;
    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::ToolUpdated { progress, .. } => {
                saw_progress = true;
                assert!(progress.text().contains("editing"));
                assert_eq!(
                    progress.presentation().operation_kind(),
                    Some(&OperationKind::Write)
                );
            }
            RunEvent::ToolFinished { result, .. } => match result {
                ToolCompletion::Success(output) => {
                    edit_metadata = Some(output.presentation().clone());
                    assert!(output.content().contains("edited sample.txt"));
                    assert!(output.content().contains("+alpha delta gamma"));
                }
                other => panic!("unexpected tool result: {other:?}"),
            },
            RunEvent::Completed { .. } => break,
            _ => {}
        }
    }

    assert!(saw_progress);
    let metadata = edit_metadata.expect("edit metadata");
    assert_eq!(metadata.operation_kind(), Some(&OperationKind::Write));
    assert_eq!(
        metadata.affected_paths(),
        [std::path::PathBuf::from("sample.txt")]
    );
    assert!(metadata.unified_diff().unwrap().contains("+alpha delta gamma"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("sample.txt")).unwrap(),
        "alpha delta gamma"
    );
}

