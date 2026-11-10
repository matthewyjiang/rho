//! Moving a session's conversation to another node of its saved tree.
//!
//! The runtime is rebuilt around the node's conversation, as a permission
//! mode change rebuilds it around the live one, and the session file's leaf
//! moves there, so later turns branch from the node and the nodes after it
//! stay in the tree. The session keeps its model, settings, tools, and
//! added directories: those belong to the session, not to a node.

use rho_sdk::SystemPrompt;

use super::{model_scoped_config, SessionRuntime, SettingChangeError};
use crate::{
    app::{
        conversation_switch::replace_system_prompt, interactive_runtime::startup::prompt_cache_key,
    },
    config::Config,
    session::tree::NodeId,
};

/// Why [`SessionRuntime::select_node`] left the conversation where it was.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SelectNodeError {
    /// Nothing is saved yet, so the session has no tree.
    #[error("the session has no saved conversation yet")]
    Unsaved,
    /// The session's tree holds no such node, or cannot be read.
    #[error("{0:#}")]
    Node(anyhow::Error),
    #[error(transparent)]
    Change(SettingChangeError),
}

impl SessionRuntime {
    /// Continues the conversation from `node` of the session's saved tree.
    /// On error the session keeps the conversation it had.
    pub(crate) async fn select_node(
        &mut self,
        node: &NodeId,
        process_config: &Config,
    ) -> Result<(), SelectNodeError> {
        let stored = self
            .storage
            .file()
            .cloned()
            .ok_or(SelectNodeError::Unsaved)?;
        let conversation = stored
            .snapshot_for_node(
                node,
                self.built.provider.identity(),
                prompt_cache_key(stored.id()),
            )
            .map_err(SelectNodeError::Node)?;
        let previous = self.built.session.snapshot();
        self.rebuild_onto(conversation, process_config)
            .await
            .map_err(SelectNodeError::Change)?;
        if let Err(error) = stored.set_leaf(node) {
            // The file still continues from the old leaf; so must the
            // runtime.
            let error = match self.rebuild_onto(previous, process_config).await {
                Ok(()) => error,
                Err(rollback) => error.context(format!("rollback failed: {rollback}")),
            };
            return Err(SelectNodeError::Change(SettingChangeError::Apply(error)));
        }
        Ok(())
    }

    /// Rebuilds the runtime around `conversation` under the system prompt
    /// the session runs with now, which a node saved before a model switch
    /// does not hold.
    async fn rebuild_onto(
        &mut self,
        conversation: rho_sdk::SessionSnapshot,
        process_config: &Config,
    ) -> Result<(), SettingChangeError> {
        let config = model_scoped_config(&self.current_model(), process_config);
        let (mode, writes) = (self.permission.mode, self.permission.writes.clone());
        self.rebuild_runtime_onto(&config, mode, writes, conversation)
            .await?;
        if let SystemPrompt::Custom(text) = &self.built.prompt.system {
            replace_system_prompt(&self.built.session, text)
                .map_err(|error| SettingChangeError::Apply(error.into()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
