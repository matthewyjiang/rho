//! Directories a host adds to a session's workspace between its turns.
//!
//! The set is saved with the session's snapshot (`persist.rs`). Changing it
//! rebuilds the SDK runtime, so the next tool call sees the new roots.

use std::path::PathBuf;

use rho_sdk::model::Message;

use super::{model_scoped_config, SessionRuntime, SettingChangeError};
use crate::{
    added_dirs::{AddedDirs, Insertion},
    config::Config,
};

/// Why [`SessionRuntime::add_dir`] added nothing.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AddDirError {
    /// The workspace already reaches the directory through this root.
    #[error("{} is already in scope through {}", crate::paths::display(.dir), crate::paths::display(.root))]
    Covered { dir: PathBuf, root: PathBuf },
    #[error(transparent)]
    Change(SettingChangeError),
}

impl SessionRuntime {
    /// The directories the session's tools reach besides its workspace.
    pub(crate) fn added_dirs(&self) -> &AddedDirs {
        &self.added_dirs
    }

    /// Adds the canonical `dir` to the session's workspace scope and saves
    /// a row saying so.
    ///
    /// The live system prompt stays fixed for prompt-cache stability; the
    /// directory and its new AGENTS.md files reach the model as added
    /// context. Later prompt rebuilds (model switch, resume) include them.
    /// On error the session keeps the directories it had.
    pub(crate) async fn add_dir(
        &mut self,
        dir: PathBuf,
        process_config: &Config,
    ) -> Result<(), AddDirError> {
        let previous = self.added_dirs.clone();
        let mut added = previous.clone();
        // `dir` is canonical, so the workspace it is checked against must
        // be: one opened through a symlink still covers what is under it.
        let workspace = &self.permission.workspace_root;
        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.clone());
        if let Insertion::Covered(root) = added.insert(&workspace, dir.clone()) {
            return Err(AddDirError::Covered { dir, root });
        }
        self.install_added_dirs(added.clone(), process_config)
            .await
            .map_err(AddDirError::Change)?;
        let new_files = self
            .built
            .prompt_template
            .as_mut()
            .map(|template| template.set_added_dirs(&added))
            .unwrap_or_default();
        // The row also advances the revision the saved set rides on.
        let (model, display) = crate::prompt::added_dir_context(&dir, &new_files);
        if let Err(error) = self.append_context(model, Message::user_text(display), []) {
            if let Some(template) = self.built.prompt_template.as_mut() {
                template.set_added_dirs(&previous);
            }
            let error = match self.install_added_dirs(previous, process_config).await {
                Ok(()) => error,
                Err(rollback) => error.context(format!("rollback failed: {rollback}")),
            };
            return Err(AddDirError::Change(SettingChangeError::Apply(error)));
        }
        Ok(())
    }

    /// Rebuilds the runtime onto a workspace granting `added`, which later
    /// subagents are launched with too. On error the session keeps the
    /// directories and the runtime it had.
    async fn install_added_dirs(
        &mut self,
        added: AddedDirs,
        process_config: &Config,
    ) -> Result<(), SettingChangeError> {
        let config = model_scoped_config(&self.current_model(), process_config);
        // The rebuild reads the session's set.
        let previous = std::mem::replace(&mut self.added_dirs, added);
        let (mode, writes) = (self.permission.mode, self.permission.writes.clone());
        if let Err(error) = self.rebuild_runtime(&config, mode, writes).await {
            self.added_dirs = previous;
            return Err(error);
        }
        if let Some(manager) = self.built.tools.subagents() {
            manager.update_added_dirs(self.added_dirs.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "added_dirs_tests.rs"]
mod tests;
