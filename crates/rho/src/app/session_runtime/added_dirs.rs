//! Directories a host adds to a session's workspace between its turns.
//!
//! The set is saved with the session's snapshot (`persist.rs`). Changing it
//! rebuilds the SDK runtime, so the next tool call sees the new roots.

use std::path::PathBuf;

use rho_sdk::model::Message;

use super::{SessionRuntime, SettingChangeError};
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
        if let Insertion::Covered(root) = added.insert(&self.permission.workspace_root, dir.clone())
        {
            return Err(AddDirError::Covered { dir, root });
        }
        let mode = self.permission.mode;
        self.rebuild_runtime(mode, added.clone(), process_config)
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
            let error = match self.rebuild_runtime(mode, previous, process_config).await {
                Ok(()) => error,
                Err(rollback) => error.context(format!("rollback failed: {rollback}")),
            };
            return Err(AddDirError::Change(SettingChangeError::Apply(error)));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "added_dirs_tests.rs"]
mod tests;
