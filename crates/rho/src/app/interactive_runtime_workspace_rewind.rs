use std::{collections::BTreeMap, path::PathBuf};

use super::*;
use crate::session::workspace_checkpoint::{
    CheckpointAppendError, CheckpointFileBudgetExceeded, CheckpointOutcome, ObservedFileState,
    RestoreClassification, UnsupportedPath,
};

#[cfg(test)]
#[path = "interactive_runtime_workspace_rewind_tests.rs"]
mod tests;

struct PreparedWorkspaceRewind {
    checkpoint: crate::session::workspace_checkpoint::WorkspaceCheckpoint,
    current: BTreeMap<PathBuf, ObservedFileState>,
    plan: crate::session::workspace_checkpoint::RestorePlan,
    conversation: tree::PreparedTreeSelection,
    display: Vec<Message>,
}

#[derive(Debug)]
pub(crate) struct WorkspaceRewindResult {
    pub(crate) audit: crate::session::workspace_checkpoint::RestoreAudit,
    pub(crate) display: Option<(crate::session::tree::NodeId, Vec<Message>)>,
    pub(crate) selection_error: Option<anyhow::Error>,
}

fn rewind_boundary(
    checkpoint: &crate::session::workspace_checkpoint::WorkspaceCheckpoint,
) -> anyhow::Result<&crate::session::tree::NodeId> {
    checkpoint.before_node_id.as_ref().ok_or_else(|| {
        anyhow::anyhow!("could not rewind workspace: checkpoint predates rewind boundaries")
    })
}

impl InteractiveRuntime {
    /// Establish a durable empty root so even the first turn has a rewind target.
    pub(super) fn begin_workspace_checkpoint(&mut self) -> anyhow::Result<()> {
        if !self.workspace_rewind
            || self
                .checkpoint_paused_sessions
                .contains(self.sessions.id().as_str())
        {
            return Ok(());
        }
        let Some(storage) = self.sessions.storage().cloned() else {
            return Ok(());
        };
        match storage.workspace_checkpoint_store() {
            Ok(None) => return Ok(()),
            Ok(Some(_)) => {}
            Err(error) => {
                self.pause_workspace_checkpoint(&storage, &error);
                return Ok(());
            }
        }
        // This baseline is conversation persistence, not checkpoint persistence.
        // Do not start a turn when its durable conversation boundary cannot be saved.
        if storage.active_checkpoint_target()?.is_none() {
            self.sessions.save_snapshot(&[])?;
        }
        match self.tools.checkpoint_tracker().begin_turn(Some(&storage)) {
            Ok(()) => Ok(()),
            Err(error)
                if matches!(
                    error.downcast_ref::<CheckpointAppendError>(),
                    Some(CheckpointAppendError::Storage(_))
                ) =>
            {
                self.pause_workspace_checkpoint(&storage, &error);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn pause_workspace_checkpoint(&mut self, storage: &StoredSession, error: &anyhow::Error) {
        self.tools.checkpoint_tracker().discard_turn();
        if self
            .checkpoint_paused_sessions
            .insert(storage.id().to_string())
        {
            self.sessions.queue_notice(format!(
                "could not start workspace checkpoint: {error}; rewind paused for this session"
            ));
        }
    }

    /// A full checkpoint budget pauses capture, not the conversation or earlier rewinds.
    pub(super) fn finalize_workspace_checkpoint(&mut self, outcome: &Result<RunOutcome, Error>) {
        let Some(storage) = self.sessions.storage().cloned() else {
            self.tools.checkpoint_tracker().discard_turn();
            return;
        };
        self.runs.mark_display_committed();
        let outcome = match outcome {
            Ok(_) => CheckpointOutcome::Completed,
            Err(Error::Cancelled | Error::Interrupted { .. }) => CheckpointOutcome::Cancelled,
            Err(_) => CheckpointOutcome::Failed,
        };
        let result = storage
            .active_checkpoint_target()
            .and_then(|target| match target {
                Some((node_id, revision)) => self
                    .tools
                    .checkpoint_tracker()
                    .finalize_turn(node_id, revision, outcome),
                None => {
                    self.tools.checkpoint_tracker().discard_turn();
                    Ok(None)
                }
            });
        if let Ok(Some(checkpoint)) = &result {
            for file in &checkpoint.files {
                if let Some(CheckpointFileBudgetExceeded { asked, limit }) =
                    file.capture_budget_exceeded()
                {
                    self.sessions.queue_notice(format!(
                        "workspace checkpoint skipped '{}': per-file capture budget is {limit} bytes, asked {asked} bytes; this file cannot be rewound",
                        file.path.display()
                    ));
                }
            }
        }
        if let Err(error) = result {
            tracing::warn!(%error, "failed to persist workspace checkpoint");
            self.tools.checkpoint_tracker().discard_turn();
            match error.downcast::<CheckpointAppendError>() {
                Ok(CheckpointAppendError::QuotaExceeded {
                    asked,
                    limit,
                    turn_bytes,
                }) => {
                    if self
                        .checkpoint_paused_sessions
                        .insert(storage.id().to_string())
                    {
                        self.sessions.queue_notice(format!(
                            "workspace checkpoints paused: this session reached its {limit} bytes checkpoint budget (asked at least {asked} bytes; turn requested at least {turn_bytes} bytes); earlier turns stay rewindable"
                        ));
                    }
                }
                Ok(CheckpointAppendError::Storage(error)) | Err(error) => {
                    self.sessions
                        .queue_notice(format!("could not save workspace checkpoint: {error}"));
                }
            }
        }
    }

    pub(crate) fn workspace_checkpoints(
        &self,
    ) -> anyhow::Result<Vec<crate::session::workspace_checkpoint::WorkspaceCheckpointSummary>> {
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
        rewind_boundary(&checkpoint)?;
        let current = self.observe_checkpoint_paths(&store, &checkpoint);
        let plan = crate::session::workspace_checkpoint::plan_restore(&checkpoint, &current)?;
        Ok((checkpoint, plan))
    }

    pub(crate) async fn restore_workspace_rewind(
        &mut self,
        target_id: &crate::session::tree::NodeId,
    ) -> anyhow::Result<WorkspaceRewindResult> {
        if self.is_session_busy() {
            anyhow::bail!(if self.runs.is_active() {
                "workspace rewind is unavailable while a provider run is active"
            } else {
                "workspace rewind is unavailable while compaction is active"
            });
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
        let before = rewind_boundary(&checkpoint)?.clone();
        let current = self.observe_checkpoint_paths(&store, &checkpoint);
        let plan = crate::session::workspace_checkpoint::plan_restore(&checkpoint, &current)?;
        let display = storage.histories_for_node(&before)?.display;
        let conversation = self
            .prepare_tree_selection(storage.clone(), &before)
            .await?;
        let prepared = PreparedWorkspaceRewind {
            checkpoint,
            current,
            plan,
            conversation,
            display,
        };
        // Preparation above is entirely fallible and makes no file changes.
        // The restore rechecks paths against this plan before each write.
        self.authorize_restore_actions(&prepared.plan)?;
        let audit = store.restore(
            &prepared.checkpoint,
            &prepared.current,
            |path| self.observe_checkpoint_path(&store, path),
            |file, classification| self.apply_checkpoint_restore(&store, file, classification),
        );
        let incomplete = audit.entries.iter().any(|entry| {
            entry.error.is_some()
                || matches!(
                    entry.classification,
                    RestoreClassification::Conflict | RestoreClassification::Unsupported
                )
        });
        if incomplete {
            return Ok(WorkspaceRewindResult {
                audit,
                display: None,
                selection_error: None,
            });
        }
        let selection_error = self
            .commit_tree_selection(prepared.conversation)
            .await
            .err();
        Ok(WorkspaceRewindResult {
            audit,
            display: selection_error
                .is_none()
                .then_some((before, prepared.display)),
            selection_error,
        })
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
