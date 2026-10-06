//! Turn-scoped native mutation capture and finalization.

use super::*;
use std::sync::{Arc, Mutex};

#[cfg(test)]
#[path = "workspace_checkpoint_tracker_tests.rs"]
mod tests;

/// Turn-scoped mutation observer shared by native workspace tools.
#[derive(Clone, Debug)]
pub(crate) struct WorkspaceCheckpointTracker {
    enabled: bool,
    active: Arc<Mutex<Option<ActiveCheckpoint>>>,
}

#[derive(Debug)]
struct ActiveCheckpoint {
    store: WorkspaceCheckpointStore,
    open: OpenWorkspaceCheckpoint,
}

impl WorkspaceCheckpointTracker {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            active: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn begin_turn(&self, session: Option<&Session>) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let Some(session) = session else {
            return Ok(());
        };
        let Some(store) = session
            .workspace_checkpoint_store()
            .map_err(CheckpointAppendError::Storage)?
        else {
            return Ok(());
        };
        // Conversation storage failures stay fatal; checkpoint storage is optional.
        let before_node_id = session.active_checkpoint_target()?.map(|(id, _)| id);
        let mut open = store
            .open(NodeId::new())
            .map_err(CheckpointAppendError::Storage)?;
        open.before_node_id = before_node_id;
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        anyhow::ensure!(
            active.is_none(),
            "a workspace checkpoint turn is already active"
        );
        *active = Some(ActiveCheckpoint { store, open });
        Ok(())
    }

    pub(crate) fn discard_turn(&self) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = None;
    }

    pub(crate) fn finalize_turn(
        &self,
        node_id: NodeId,
        revision: Revision,
        outcome: CheckpointOutcome,
    ) -> anyhow::Result<Option<WorkspaceCheckpoint>> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(active) = active else {
            return Ok(None);
        };
        active
            .store
            .finalize_for_node(active.open, node_id, revision, outcome)
            .map(Some)
    }
}

impl rho_tools::WorkspaceMutationObserver for WorkspaceCheckpointTracker {
    fn before_mutation<'a>(
        &'a self,
        paths: &'a [&'a Path],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        let result = {
            let mut active = self
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(active) = active.as_mut() {
                for path in paths {
                    active.open.capture_path(path);
                }
            }
            Ok(())
        };
        Box::pin(std::future::ready(result))
    }

    fn after_mutation<'a>(
        &'a self,
        paths: &'a [&'a Path],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        let result = {
            let mut active = self
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(active) = active
                .as_mut()
                .filter(|active| active.open.quota_exceeded.is_none())
            {
                for path in paths {
                    if !active.open.reserve_expected_after(path) {
                        continue;
                    }
                    let state = active.store.observe_path(path);
                    active
                        .open
                        .expected_after
                        .insert((*path).to_path_buf(), state);
                }
            }
            Ok(())
        };
        Box::pin(std::future::ready(result))
    }

    fn mark_untracked_effect(&self, kind: rho_tools::UntrackedWorkspaceEffect, source: &str) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(active) = active.as_mut() else {
            return;
        };
        let effect = UntrackedEffect {
            kind: match kind {
                rho_tools::UntrackedWorkspaceEffect::ShellCommand => {
                    UntrackedEffectKind::ShellCommand
                }
                rho_tools::UntrackedWorkspaceEffect::MutatingTool => {
                    UntrackedEffectKind::UntrackedMutatingTool
                }
            },
            source: source.to_string(),
        };
        active.open.record_untracked_effect(effect);
    }
}
