use std::{collections::BTreeMap, path::PathBuf};

use super::*;
use crate::session::workspace_checkpoint::{
    ObservedFileState, RestoreClassification, UnsupportedPath,
};

impl InteractiveRuntime {
    pub(crate) fn workspace_checkpoints(
        &self,
    ) -> anyhow::Result<Vec<crate::session::workspace_checkpoint::WorkspaceCheckpoint>> {
        let storage = self
            .sessions
            .storage()
            .ok_or_else(|| anyhow::anyhow!("active session storage is unavailable"))?;
        let Some(store) = storage.workspace_checkpoint_store()? else {
            return Ok(Vec::new());
        };
        store.list()
    }

    pub(crate) fn preview_workspace_rewind(
        &self,
        target_id: &crate::session::tree::NodeId,
    ) -> anyhow::Result<(
        crate::session::workspace_checkpoint::WorkspaceCheckpoint,
        crate::session::workspace_checkpoint::RestorePlan,
    )> {
        let storage = self
            .sessions
            .storage()
            .ok_or_else(|| anyhow::anyhow!("active session storage is unavailable"))?;
        let store = storage
            .workspace_checkpoint_store()?
            .ok_or_else(|| anyhow::anyhow!("workspace rewind is unavailable for this session"))?;
        let checkpoint = store
            .get(target_id)?
            .ok_or_else(|| anyhow::anyhow!("workspace checkpoint '{target_id}' was not found"))?;
        let current = self.observe_checkpoint_paths(&store, &checkpoint);
        let plan = crate::session::workspace_checkpoint::plan_restore(&checkpoint, &current)?;
        Ok((checkpoint, plan))
    }

    pub(crate) async fn restore_workspace_rewind(
        &mut self,
        target_id: &crate::session::tree::NodeId,
    ) -> anyhow::Result<crate::session::workspace_checkpoint::RestoreAudit> {
        if self.runs.is_active() {
            anyhow::bail!("workspace rewind is unavailable while a provider run is active");
        }
        if self.permission_mode == PermissionMode::Plan {
            anyhow::bail!("workspace rewind is unavailable in plan permission mode");
        }
        let storage = self
            .sessions
            .storage()
            .ok_or_else(|| anyhow::anyhow!("active session storage is unavailable"))?;
        let store = storage
            .workspace_checkpoint_store()?
            .ok_or_else(|| anyhow::anyhow!("workspace rewind is unavailable for this session"))?;
        let checkpoint = store
            .get(target_id)?
            .ok_or_else(|| anyhow::anyhow!("workspace checkpoint '{target_id}' was not found"))?;
        let current = self.observe_checkpoint_paths(&store, &checkpoint);
        let plan = crate::session::workspace_checkpoint::plan_restore(&checkpoint, &current)?;
        self.authorize_restore_actions(&plan)?;
        Ok(store.restore(
            &checkpoint,
            &current,
            |path| self.observe_checkpoint_path(&store, path),
            |file, classification| self.apply_checkpoint_restore(&store, file, classification),
        ))
    }

    fn observe_checkpoint_paths(
        &self,
        store: &crate::session::workspace_checkpoint::WorkspaceCheckpointStore,
        checkpoint: &crate::session::workspace_checkpoint::WorkspaceCheckpoint,
    ) -> BTreeMap<PathBuf, ObservedFileState> {
        checkpoint
            .files
            .iter()
            .map(|file| {
                (
                    file.path.clone(),
                    self.observe_checkpoint_path(store, &file.path),
                )
            })
            .collect()
    }

    fn observe_checkpoint_path(
        &self,
        store: &crate::session::workspace_checkpoint::WorkspaceCheckpointStore,
        path: &std::path::Path,
    ) -> ObservedFileState {
        match self.workspace.resolve_for_write(path) {
            Ok(resolved)
                if resolved.path() == path && self.workspace.revalidate(&resolved).is_ok() =>
            {
                store.observe_path(path)
            }
            _ => ObservedFileState::Unsupported {
                reason: UnsupportedPath::Unreadable,
            },
        }
    }

    fn apply_checkpoint_restore(
        &self,
        store: &crate::session::workspace_checkpoint::WorkspaceCheckpointStore,
        file: &crate::session::workspace_checkpoint::FileCheckpoint,
        classification: RestoreClassification,
    ) -> anyhow::Result<()> {
        let resolved = self.workspace.resolve_for_write(&file.path)?;
        anyhow::ensure!(
            resolved.path() == file.path,
            "checkpoint path '{}' resolves to a different workspace path",
            file.path.display()
        );
        self.workspace.revalidate(&resolved)?;
        store.apply_restore(file, classification)
    }

    fn authorize_restore_actions(
        &self,
        plan: &crate::session::workspace_checkpoint::RestorePlan,
    ) -> anyhow::Result<()> {
        for entry in &plan.entries {
            if !matches!(
                entry.classification,
                RestoreClassification::Create
                    | RestoreClassification::Modify
                    | RestoreClassification::Delete
            ) {
                continue;
            }
            let resolved = self.workspace.resolve_for_write(&entry.path)?;
            anyhow::ensure!(
                resolved.path() == entry.path,
                "checkpoint path '{}' resolves to a different workspace path",
                entry.path.display()
            );
            self.workspace.revalidate(&resolved)?;
        }
        Ok(())
    }
}
