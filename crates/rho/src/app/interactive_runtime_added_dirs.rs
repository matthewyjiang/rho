//! Added workspace directories as interactive session state.
//!
//! The live set belongs to the session controller, which saves it in snapshot
//! metadata. Every session starts from the `--add-dir` directories; a resumed
//! session adds back the directories it saved. Changing the set rebuilds the
//! SDK runtime so the next tool call sees the new granted roots.

use std::path::PathBuf;

use super::InteractiveRuntime;
use crate::added_dirs::{AddedDirs, Insertion};

/// Result of [`InteractiveRuntime::add_dir`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AddDirOutcome {
    Added,
    /// Already in scope through this root.
    AlreadyCovered(PathBuf),
}

impl InteractiveRuntime {
    pub(crate) fn added_dirs(&self) -> &AddedDirs {
        self.sessions.added_dirs()
    }

    /// Adds a canonical directory to the current session's workspace scope.
    ///
    /// The live system prompt stays fixed for prompt-cache stability; the
    /// directory and its new AGENTS.md files reach the model as appended
    /// context. Later prompt rebuilds (model switch, resume) include them.
    pub(crate) async fn add_dir(&mut self, dir: PathBuf) -> anyhow::Result<AddDirOutcome> {
        if self.is_session_busy() {
            anyhow::bail!("directories cannot be added while a run or compaction is active");
        }
        let previous_dirs = self.sessions.added_dirs().clone();
        let mut added = previous_dirs.clone();
        if let Insertion::Covered(root) = added.insert(self.workspace.root(), dir.clone()) {
            return Ok(AddDirOutcome::AlreadyCovered(root));
        }
        self.install_added_dirs(added.clone()).await?;
        let new_files = self
            .prompt_template
            .as_mut()
            .map(|template| template.set_added_dirs(&added))
            .unwrap_or_default();
        // Always append context, even without a system prompt: the message
        // advances the revision, and a metadata-only save at an unchanged
        // revision is rejected by session-tree restore.
        let (model, display) = crate::prompt::added_dir_context(&dir, &new_files);
        if let Err(error) = self.append_user_context_with_display(model, display) {
            if let Some(template) = self.prompt_template.as_mut() {
                template.set_added_dirs(&previous_dirs);
            }
            if let Err(rollback) = self.install_added_dirs(previous_dirs).await {
                return Err(error.context(format!("rollback failed: {rollback}")));
            }
            return Err(error);
        }
        Ok(AddDirOutcome::Added)
    }

    /// Rebinds the live session onto a workspace granting `added`, then
    /// records the set on the session and for future delegated launches.
    async fn install_added_dirs(&mut self, added: AddedDirs) -> anyhow::Result<()> {
        let workspace = self.workspace_with(&added)?;
        let previous = std::mem::replace(&mut self.workspace, workspace);
        if let Err(error) = self.rebind_current_session().await {
            self.workspace = previous;
            return Err(error);
        }
        self.adopt_added_dirs(added);
        Ok(())
    }

    /// The directories a session starts with: launch directories plus any
    /// `storage` saved. Missing saved directories are reported.
    pub(super) fn starting_added_dirs(
        &self,
        storage: &crate::session::Session,
    ) -> (AddedDirs, Vec<PathBuf>) {
        let root = self.workspace.root();
        let restored = AddedDirs::from_storage(storage, root);
        (
            self.launch_added_dirs.union(root, &restored.dirs),
            restored.missing,
        )
    }

    /// The primary workspace with `added` granted on top.
    pub(super) fn workspace_with(&self, added: &AddedDirs) -> anyhow::Result<rho_sdk::Workspace> {
        Ok(crate::app::sdk_config::WorkspaceOptions {
            root: self.workspace.root().to_path_buf(),
        }
        .build_workspace(added)?)
    }

    /// Records `added` as the session's set once its workspace is installed.
    pub(super) fn adopt_added_dirs(&mut self, added: AddedDirs) {
        if let Some(manager) = self.tools.subagents() {
            manager.update_added_dirs(added.clone());
        }
        self.sessions.set_added_dirs(added);
    }

    pub(super) fn queue_missing_added_dirs_notice(&mut self, missing: &[PathBuf]) {
        for dir in missing {
            self.sessions.queue_notice(format!(
                "could not restore added directory {}: not found",
                crate::paths::display(dir)
            ));
        }
    }
}
