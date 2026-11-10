//! Workspace checkpoints of a session's turns, and rewinding to one.
//!
//! A turn's file writes are captured through the tool set's checkpoint
//! tracker between [`SessionRuntime::begin_workspace_checkpoint`] and
//! [`SessionRuntime::finalize_workspace_checkpoint`], for the sessions a
//! person prompts; a subagent's writes belong to its parent's turn. A
//! rewind restores the files a turn changed and moves the conversation to
//! the node before it (`tree.rs`).

use super::{build::SessionRole, SelectNodeError, SessionRuntime};
use crate::{
    app::{
        sdk_config::WorkspaceOptions,
        workspace_rewind::{restored_fully, rewind_boundary, PlannedRewind},
    },
    config::Config,
    permission::PermissionMode,
    session::{
        tree::NodeId,
        workspace_checkpoint::{
            CheckpointAppendError, CheckpointFileBudgetExceeded, CheckpointOutcome, RestoreAudit,
        },
    },
};

/// What [`SessionRuntime::rewind_workspace`] did.
#[derive(Debug)]
pub(crate) struct WorkspaceRewind {
    /// What became of each path the checkpoint covers.
    pub(crate) audit: RestoreAudit,
    pub(crate) conversation: RewoundConversation,
}

/// Where the conversation stands after a rewind's files were restored.
#[derive(Debug)]
pub(crate) enum RewoundConversation {
    /// It continues from before the rewound turn.
    Restored,
    /// Some path was not restored, so it stays where it was.
    Kept,
    /// Every path was restored, but the conversation could not be moved.
    Failed(SelectNodeError),
}

/// Why [`SessionRuntime::rewind_workspace`] changed nothing.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RewindError {
    /// The session cannot rewind, or not to this checkpoint; the requester
    /// can tell why.
    #[error("{0:#}")]
    Refused(anyhow::Error),
    /// The checkpoints or the workspace could not be read or written.
    #[error("{0:#}")]
    Failed(anyhow::Error),
}

impl SessionRuntime {
    /// Whether this session's turns take workspace checkpoints.
    fn takes_checkpoints(&self) -> bool {
        match self.build.run.role {
            SessionRole::Interactive | SessionRole::Daemon | SessionRole::Run => {
                !self.checkpoints_paused
            }
            // Their writes are their parent's turn's, or no session's.
            SessionRole::Subagent { .. } | SessionRole::WorkflowNode => false,
        }
    }

    /// Opens the coming turn's checkpoint at the conversation's leaf, which
    /// the turn's checkpoint store has saved by now. Returns a notice when
    /// checkpoint storage failed, which pauses capture for the session and
    /// never the turn.
    pub(super) fn begin_workspace_checkpoint(&mut self) -> anyhow::Result<Option<String>> {
        if !self.takes_checkpoints() {
            return Ok(None);
        }
        let Some(stored) = self.storage.file() else {
            return Ok(None);
        };
        // One a turn that panicked left open would refuse this one.
        self.built.tools.checkpoint_tracker().discard_turn();
        match self
            .built
            .tools
            .checkpoint_tracker()
            .begin_turn(Some(stored))
        {
            Ok(()) => Ok(None),
            Err(error)
                if matches!(
                    error.downcast_ref::<CheckpointAppendError>(),
                    Some(CheckpointAppendError::Storage(_))
                ) =>
            {
                self.built.tools.checkpoint_tracker().discard_turn();
                self.checkpoints_paused = true;
                Ok(Some(format!(
                    "could not start workspace checkpoint: {error}; rewind paused for this session"
                )))
            }
            Err(error) => Err(error),
        }
    }

    /// Saves the ended turn's checkpoint against the conversation's leaf,
    /// and returns what to tell the session's clients. A full checkpoint
    /// budget pauses capture, not the conversation or earlier rewinds.
    pub(super) fn finalize_workspace_checkpoint(
        &mut self,
        outcome: CheckpointOutcome,
    ) -> Vec<String> {
        let tracker = self.built.tools.checkpoint_tracker().clone();
        let Some(stored) = self.storage.file() else {
            tracker.discard_turn();
            return Vec::new();
        };
        let saved = stored
            .active_checkpoint_target()
            .and_then(|target| match target {
                Some((node, revision)) => tracker.finalize_turn(node, revision, outcome),
                None => {
                    tracker.discard_turn();
                    Ok(None)
                }
            });
        match saved {
            Ok(None) => Vec::new(),
            Ok(Some(checkpoint)) => checkpoint
                .files
                .iter()
                .filter_map(|file| {
                    let CheckpointFileBudgetExceeded { asked, limit } =
                        file.capture_budget_exceeded()?;
                    Some(format!(
                        "workspace checkpoint skipped '{}': per-file capture budget is {limit} bytes, asked {asked} bytes; this file cannot be rewound",
                        file.path.display()
                    ))
                })
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "failed to persist workspace checkpoint");
                tracker.discard_turn();
                match error.downcast::<CheckpointAppendError>() {
                    Ok(CheckpointAppendError::QuotaExceeded {
                        asked,
                        limit,
                        turn_bytes,
                    }) => {
                        self.checkpoints_paused = true;
                        vec![format!(
                            "workspace checkpoints paused: this session reached its {limit} bytes checkpoint budget (asked at least {asked} bytes; turn requested at least {turn_bytes} bytes); earlier turns stay rewindable"
                        )]
                    }
                    Ok(CheckpointAppendError::Storage(error)) | Err(error) => {
                        vec![format!("could not save workspace checkpoint: {error}")]
                    }
                }
            }
        }
    }

    /// Restores the files checkpoint `target`'s turn changed and, when
    /// every one was restored, continues the conversation from before that
    /// turn. A session or checkpoint that cannot rewind is refused before a
    /// file is written; moving the conversation can still fail after, which
    /// [`RewoundConversation::Failed`] says.
    pub(crate) async fn rewind_workspace(
        &mut self,
        target: &NodeId,
        process_config: &Config,
    ) -> Result<WorkspaceRewind, RewindError> {
        use RewindError::{Failed, Refused};
        if self.permission.mode == PermissionMode::Plan {
            return Err(Refused(anyhow::anyhow!(
                "workspace rewind is unavailable in plan permission mode"
            )));
        }
        let stored =
            self.storage.file().cloned().ok_or_else(|| {
                Refused(anyhow::anyhow!("the session has no saved conversation yet"))
            })?;
        let store = stored
            .workspace_checkpoint_store()
            .map_err(Failed)?
            .ok_or_else(|| {
                Refused(anyhow::anyhow!(
                    "workspace rewind is unavailable for this session"
                ))
            })?;
        let workspace = WorkspaceOptions {
            root: self.permission.workspace_root.clone(),
        }
        .build_workspace(&self.added_dirs)
        .map_err(|error| Failed(error.into()))?;
        let planned = PlannedRewind::new(&store, &workspace, target).map_err(Refused)?;
        let before = rewind_boundary(&planned.checkpoint)
            .map_err(Refused)?
            .clone();
        // The node must still be there to return to once files changed.
        stored.histories_for_node(&before).map_err(Refused)?;
        let audit = planned.restore(&store, &workspace).map_err(Failed)?;
        let conversation = if restored_fully(&audit) {
            match self.select_node(&before, process_config).await {
                Ok(()) => RewoundConversation::Restored,
                Err(error) => RewoundConversation::Failed(error),
            }
        } else {
            RewoundConversation::Kept
        };
        Ok(WorkspaceRewind {
            audit,
            conversation,
        })
    }
}

#[cfg(test)]
#[path = "workspace_rewind_tests.rs"]
mod tests;
