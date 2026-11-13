//! Rewinding a workspace to a checkpoint: the steps every host of a
//! session shares, over the workspace its tools write through, and how the
//! result reads.
//!
//! A checkpoint path is restored only where the workspace would let a tool
//! write it now, and is checked again right before each write.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use rho_sdk::Workspace;

use crate::session::{
    tree::NodeId,
    workspace_checkpoint::{
        plan_restore, FileCheckpoint, ObservedFileState, RestoreAudit, RestoreClassification,
        RestorePlan, UnsupportedPath, UntrackedEffectKind, WorkspaceCheckpoint,
        WorkspaceCheckpointStore,
    },
};

/// The node the conversation returns to when `checkpoint`'s turn is
/// rewound.
pub(crate) fn rewind_boundary(checkpoint: &WorkspaceCheckpoint) -> anyhow::Result<&NodeId> {
    checkpoint.before_node_id.as_ref().ok_or_else(|| {
        anyhow::anyhow!("could not rewind workspace: checkpoint predates rewind boundaries")
    })
}

/// A rewind of one checkpoint, planned against the workspace as it is.
pub(crate) struct PlannedRewind {
    pub(crate) checkpoint: WorkspaceCheckpoint,
    /// What each of the checkpoint's paths held when the plan was made.
    current: BTreeMap<PathBuf, ObservedFileState>,
    pub(crate) plan: RestorePlan,
}

impl PlannedRewind {
    /// Plans the rewind of checkpoint `target` in `store`; nothing is
    /// written.
    pub(crate) fn new(
        store: &WorkspaceCheckpointStore,
        workspace: &Workspace,
        target: &NodeId,
    ) -> anyhow::Result<Self> {
        let checkpoint = store
            .get(target)?
            .ok_or_else(|| anyhow::anyhow!("workspace checkpoint '{target}' was not found"))?;
        rewind_boundary(&checkpoint)?;
        let current = checkpoint
            .files
            .iter()
            .map(|file| (file.path.clone(), observe(store, workspace, &file.path)))
            .collect();
        let plan = plan_restore(&checkpoint, &current)?;
        Ok(Self {
            checkpoint,
            current,
            plan,
        })
    }

    /// Restores the planned paths, each checked against the plan again
    /// before its write. Fails before any write when the workspace no
    /// longer lets a planned path be written.
    pub(crate) fn restore(
        &self,
        store: &WorkspaceCheckpointStore,
        workspace: &Workspace,
    ) -> anyhow::Result<RestoreAudit> {
        for entry in &self.plan.entries {
            match entry.classification {
                RestoreClassification::Create
                | RestoreClassification::Modify
                | RestoreClassification::Delete => writable(workspace, &entry.path)?,
                RestoreClassification::Conflict
                | RestoreClassification::Unsupported
                | RestoreClassification::Skipped => {}
            }
        }
        Ok(store.restore(
            &self.checkpoint,
            &self.current,
            |path| observe(store, workspace, path),
            |file: &FileCheckpoint, classification| {
                writable(workspace, &file.path)?;
                store.apply_restore(file, classification)
            },
        ))
    }
}

/// Whether every path `audit` covers was restored or needed nothing.
pub(crate) fn restored_fully(audit: &RestoreAudit) -> bool {
    !audit.entries.iter().any(|entry| {
        entry.error.is_some()
            || matches!(
                entry.classification,
                RestoreClassification::Conflict | RestoreClassification::Unsupported
            )
    })
}

/// Fails unless `workspace` lets a tool write `path` as it is spelled.
fn writable(workspace: &Workspace, path: &Path) -> anyhow::Result<()> {
    let resolved = workspace.resolve_for_write(path)?;
    anyhow::ensure!(
        resolved.path() == path,
        "checkpoint path '{}' resolves to a different workspace path",
        path.display()
    );
    workspace.revalidate(&resolved)?;
    Ok(())
}

fn observe(
    store: &WorkspaceCheckpointStore,
    workspace: &Workspace,
    path: &Path,
) -> ObservedFileState {
    match writable(workspace, path) {
        Ok(()) => store.observe_path(path),
        Err(_) => ObservedFileState::Unsupported {
            reason: UnsupportedPath::Unreadable,
        },
    }
}

pub(crate) fn rewind_audit_text(
    audit: &RestoreAudit,
    workspace: &Path,
    conversation_selected: bool,
) -> String {
    let mut lines = vec![if conversation_selected {
        "workspace rewind audit; conversation restored to before the turn".into()
    } else {
        "workspace rewind audit; conversation state was not selected".into()
    }];
    for entry in &audit.entries {
        let result =
            entry
                .error
                .as_deref()
                .unwrap_or(if entry.changed { "restored" } else { "skipped" });
        lines.push(format!(
            "{}  {}  {result}",
            classification_label(entry.classification),
            rho_tools::compact_display_path(workspace, &entry.path.to_string_lossy())
        ));
    }
    for limitation in &audit.limitations {
        lines.push(format!(
            "warning  {} effects from '{}' were not restored",
            limitation_label(&limitation.kind),
            limitation.source
        ));
    }
    lines.join("\n")
}

pub(crate) fn classification_label(classification: RestoreClassification) -> &'static str {
    match classification {
        RestoreClassification::Create => "create",
        RestoreClassification::Modify => "modify",
        RestoreClassification::Delete => "delete",
        RestoreClassification::Conflict => "conflict",
        RestoreClassification::Unsupported => "unsupported",
        RestoreClassification::Skipped => "skip",
    }
}

pub(crate) fn limitation_label(kind: &UntrackedEffectKind) -> &'static str {
    match kind {
        UntrackedEffectKind::ShellCommand => "shell",
        UntrackedEffectKind::UntrackedMutatingTool => "process",
        UntrackedEffectKind::ExternalEffect => "external",
    }
}
