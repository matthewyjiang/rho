use std::fs;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelResponse},
    provider::ScriptedTurn,
};
use rho_tools::WorkspaceMutationObserver;

use super::*;
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    session::workspace_checkpoint::WorkspaceCheckpointTracker,
    tools::sdk_registry::ToolSetOptions,
};

async fn checkpoint_runtime(
    temp: &tempfile::TempDir,
    turns: Vec<ScriptedTurn>,
) -> anyhow::Result<(InteractiveRuntime, StoredSession)> {
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace)?;
    let storage = StoredSession::create_in_root(&temp.path().join("sessions"), &workspace)?;
    let mut runtime = super::super::tests::test_runtime(turns).await;
    runtime.workspace = Workspace::new(&workspace)?;
    runtime.permission_mode = PermissionMode::Bypass;
    runtime.workspace_rewind = true;
    runtime.config.workspace_rewind = true;
    runtime.config.codemode.mode = crate::config::CodemodeMode::On;
    runtime.tools = AppToolSet::new(
        &runtime.config,
        runtime.diagnostics.clone(),
        ToolSetOptions::new(AgentCapabilities::new([ToolCapability::WriteFile].into())),
    );
    runtime.cached_tool_specs = runtime.tools.specs();
    let session = runtime
        .runtime
        .session(SessionOptions::new().id(SessionId::from_string(storage.id())?))
        .await?;
    runtime.sessions.replace_session(session, None);
    runtime.attach_storage(storage.clone());
    runtime.sessions.save_snapshot(&[])?;
    let (baseline, _) = storage.active_checkpoint_target()?.unwrap();
    runtime.select_tree_node(storage.clone(), &baseline).await?;
    Ok((runtime, storage))
}

// Covers: broken optional checkpoint storage must not block submit or native writes.
// Owner: runtime turn startup; PTY cannot cheaply inject storage failures here.
#[tokio::test]
async fn checkpoint_initialization_failure_pauses_capture_but_turns_continue() -> anyhow::Result<()>
{
    enum Case {
        JournalDirectory,
        StorageFile,
        #[cfg(unix)]
        StorageSymlink,
        #[cfg(unix)]
        JournalSymlink,
    }
    let cases = [
        Case::JournalDirectory,
        Case::StorageFile,
        #[cfg(unix)]
        Case::StorageSymlink,
        #[cfg(unix)]
        Case::JournalSymlink,
    ];
    for case in cases {
        let temp = tempfile::tempdir()?;
        let turns = vec![
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "write-1".into(),
                    name: "write".into(),
                    arguments: serde_json::json!({"path":"tracked.txt", "content":"written"}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "again".into(),
            )])),
        ];
        let (mut runtime, storage) = checkpoint_runtime(&temp, turns).await?;
        let directory = storage
            .path()
            .parent()
            .unwrap()
            .join("workspace-checkpoints");
        match case {
            Case::JournalDirectory => fs::create_dir_all(directory.join("checkpoints.jsonl"))?,
            Case::StorageFile => fs::write(&directory, b"not a directory")?,
            #[cfg(unix)]
            Case::StorageSymlink => std::os::unix::fs::symlink(temp.path(), &directory)?,
            #[cfg(unix)]
            Case::JournalSymlink => {
                fs::create_dir(&directory)?;
                std::os::unix::fs::symlink(storage.path(), directory.join("checkpoints.jsonl"))?;
            }
        }
        runtime.start(UserInput::text("write a file"), None).await?;
        assert!(runtime.is_run_active());
        assert!(runtime.checkpoint_paused_sessions.contains(storage.id()));
        assert_eq!(runtime.take_notices().len(), 1);
        runtime.finish_run().await?;
        assert_eq!(
            fs::read(temp.path().join("workspace/tracked.txt"))?,
            b"written"
        );
        runtime.start(UserInput::text("continue"), None).await?;
        runtime.finish_run().await?;
        assert!(runtime.take_notices().is_empty());
    }
    Ok(())
}

// Covers: files restored before a failed durable leaf commit must not strand retry.
// Owner: runtime rewind transaction, with failure injected by conversation storage.
#[tokio::test]
async fn rewind_retries_after_failed_durable_leaf_commit() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let (mut runtime, storage) = checkpoint_runtime(&temp, Vec::new()).await?;
    let workspace = temp.path().join("workspace");
    let added = workspace.join("added.txt");
    let deleted = workspace.join("deleted.txt");
    let modified = workspace.join("modified.txt");
    fs::write(&deleted, b"deleted original")?;
    fs::write(&modified, b"modified original")?;
    let baseline = storage.active_checkpoint_target()?;
    runtime.begin_workspace_checkpoint()?;
    let tracker = runtime.tools.checkpoint_tracker().clone();
    let paths = [added.as_path(), deleted.as_path(), modified.as_path()];
    tracker
        .before_mutation(&paths)
        .await
        .map_err(anyhow::Error::msg)?;
    fs::write(&added, b"agent added")?;
    fs::remove_file(&deleted)?;
    fs::write(&modified, b"agent modified")?;
    tracker
        .after_mutation(&paths)
        .await
        .map_err(anyhow::Error::msg)?;
    runtime
        .sessions
        .session()
        .append_message(Message::user_text("turn prompt"))?;
    runtime
        .sessions
        .save_snapshot(&[Message::user_text("turn prompt")])?;
    let target = storage.active_checkpoint_target()?.unwrap();
    tracker.finalize_turn(target.0.clone(), target.1, CheckpointOutcome::Completed)?;
    let history = runtime.history();
    let durable_before = fs::read(storage.path())?;

    storage.fail_next_leaf_commit_for_tests();
    let first = runtime.restore_workspace_rewind(&target.0).await?;
    assert!(first.selection_error.is_some());
    assert!(first.display.is_none());
    assert_eq!(runtime.history(), history);
    assert_eq!(storage.active_checkpoint_target()?, Some(target.clone()));
    assert_eq!(fs::read(storage.path())?, durable_before);
    assert_eq!(
        first
            .audit
            .entries
            .iter()
            .map(|entry| (entry.classification, entry.changed, entry.error.is_some()))
            .collect::<Vec<_>>(),
        vec![
            (RestoreClassification::Delete, true, false),
            (RestoreClassification::Create, true, false),
            (RestoreClassification::Modify, true, false)
        ],
    );
    assert!(!added.exists());
    assert_eq!(fs::read(&deleted)?, b"deleted original");
    assert_eq!(fs::read(&modified)?, b"modified original");

    let retry = runtime.restore_workspace_rewind(&target.0).await?;
    assert!(retry.selection_error.is_none());
    assert_eq!(
        retry.display,
        Some((baseline.as_ref().unwrap().0.clone(), Vec::new()))
    );
    assert_eq!(storage.active_checkpoint_target()?, baseline);
    assert_eq!(runtime.history(), Vec::<Message>::new());
    assert_eq!(
        retry
            .audit
            .entries
            .iter()
            .map(|entry| (entry.classification, entry.changed, entry.error.is_some()))
            .collect::<Vec<_>>(),
        vec![(RestoreClassification::Skipped, false, false); paths.len()],
    );
    assert!(!added.exists());
    assert_eq!(fs::read(&deleted)?, b"deleted original");
    assert_eq!(fs::read(&modified)?, b"modified original");
    Ok(())
}

// Covers: legacy checkpoints cannot infer a boundary from a parent that may
// contain the prompt; failed prompt preparation must precede all file writes.
// Owner: runtime rewind transaction, not picker rendering.
#[tokio::test]
async fn rewind_preparation_failures_preserve_files_and_conversation() -> anyhow::Result<()> {
    enum Case {
        LegacyRoot,
        LegacyChild,
        InvalidPrompt,
    }
    for case in [Case::LegacyRoot, Case::LegacyChild, Case::InvalidPrompt] {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace)?;
        let path = workspace.join("tracked.txt");
        fs::write(&path, b"original")?;
        let storage = StoredSession::create_in_root(&temp.path().join("sessions"), &workspace)?;
        let mut runtime = super::super::tests::test_runtime(Vec::new()).await;
        runtime.workspace = Workspace::new(&workspace)?;
        runtime.permission_mode = PermissionMode::Bypass;
        let session = runtime
            .runtime
            .session(SessionOptions::new().id(SessionId::from_string(storage.id())?))
            .await?;
        runtime.sessions.replace_session(session, None);
        runtime.attach_storage(storage.clone());
        runtime.sessions.save_snapshot(&[])?;
        let store = storage.workspace_checkpoint_store()?.unwrap();
        let tracker = WorkspaceCheckpointTracker::new(true);
        let legacy = if matches!(case, Case::InvalidPrompt) {
            tracker.begin_turn(Some(&storage))?;
            tracker
                .before_mutation(&[path.as_path()])
                .await
                .map_err(anyhow::Error::msg)?;
            None
        } else {
            let mut open = store.open(crate::session::tree::NodeId::new())?;
            open.capture_path(&path);
            Some(open)
        };
        if !matches!(case, Case::LegacyRoot) {
            runtime
                .sessions
                .session()
                .append_message(Message::user_text("turn prompt"))?;
            runtime
                .sessions
                .save_snapshot(&[Message::user_text("turn prompt")])?;
        }
        fs::write(&path, b"agent")?;
        let (target, revision) = storage.active_checkpoint_target()?.unwrap();
        if let Some(open) = legacy {
            store.finalize_for_node(
                open,
                target.clone(),
                revision,
                CheckpointOutcome::Completed,
            )?;
        } else {
            tracker
                .after_mutation(&[path.as_path()])
                .await
                .map_err(anyhow::Error::msg)?;
            tracker.finalize_turn(target.clone(), revision, CheckpointOutcome::Completed)?;
            let prompt_dir = temp.path().join(".rho/model-prompts");
            fs::create_dir_all(&prompt_dir)?;
            let prompt_path = prompt_dir.join("selected.md");
            fs::write(
                &prompt_path,
                "---\nprovider: test\nmodel: test\n---\noverlay",
            )?;
            let model = crate::model_identity::PromptModel::from_sdk_identity(
                &runtime.provider.provider().identity(),
            );
            let template = crate::prompt::system_prompt_template_with_home_and_models(
                &[],
                &workspace,
                Some(temp.path()),
                crate::prompt::PromptModels {
                    running: &model,
                    advisor: None,
                },
            );
            template.build(&model)?;
            runtime.prompt_template = Some(template);
            fs::write(prompt_path, "invalid frontmatter")?;
        }
        let history = runtime.history();
        let leaf = storage.active_checkpoint_target()?;
        if !matches!(case, Case::InvalidPrompt) {
            assert!(runtime.preview_workspace_rewind(&target).is_err());
        }
        let error = runtime.restore_workspace_rewind(&target).await.unwrap_err();
        if matches!(case, Case::InvalidPrompt) {
            assert!(matches!(
                error.downcast_ref::<Error>(),
                Some(Error::InvalidConfiguration { .. })
            ));
        }
        assert_eq!(fs::read(&path)?, b"agent");
        assert_eq!(runtime.history(), history);
        assert_eq!(storage.active_checkpoint_target()?, leaf);
    }
    Ok(())
}
