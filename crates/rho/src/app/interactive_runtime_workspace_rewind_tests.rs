use std::fs;

use pretty_assertions::assert_eq;
use rho_tools::WorkspaceMutationObserver;

use super::*;
use crate::session::workspace_checkpoint::WorkspaceCheckpointTracker;

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
