use std::collections::HashMap;

use rho_protocol::SessionChange;

use crate::tui::DefaultTerminal;

use crate::{
    app::{
        sdk_config::WorkspaceOptions,
        workspace_rewind::{
            classification_label, limitation_label, rewind_audit_text, PlannedRewind,
        },
    },
    commands::CommandInvocation,
    session::{
        tree::NodeId,
        workspace_checkpoint::{
            CheckpointFileBudgetExceeded, RestoreClassification, RestorePlan, WorkspaceCheckpoint,
            WorkspaceCheckpointStore,
        },
    },
};

use super::{
    picker::OverlayChrome, App, ComposerMode, Entry, InteractiveRuntime, PickerBadge,
    PickerBadgeTone, PickerItem, PickerLayout, UiPicker,
};

impl App {
    /// The followed daemon session's checkpoint store, or `None` when it
    /// has saved nothing or keeps no checkpoints.
    fn daemon_checkpoint_store(&self) -> anyhow::Result<Option<WorkspaceCheckpointStore>> {
        match self.current_session_id() {
            Some(id) => crate::session::Session::workspace_checkpoint_store_by_id(
                &self.info.runtime.cwd,
                &id,
            ),
            None => Ok(None),
        }
    }

    pub(super) fn execute_rewind_command(
        &mut self,
        invocation: CommandInvocation,
        agent: &InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.info.session.no_save {
            self.set_status("workspace rewind unavailable with --no-save");
            return Ok(());
        }
        let (checkpoints, prompts) = if self.info.session.daemon.is_some() {
            // The daemon holds the session open; its checkpoints and tree
            // are read from the files both share.
            let checkpoints = match self.daemon_checkpoint_store()? {
                Some(store) => store.list()?,
                None => Vec::new(),
            };
            // A turn's checkpoint names the node the turn ended on, which
            // later saves, such as a settings change, may follow.
            let prompts = match self.current_session_id() {
                Some(id) => crate::session::Session::turn_prompts_by_id(
                    &self.info.runtime.cwd,
                    &id,
                    checkpoints
                        .iter()
                        .map(|checkpoint| checkpoint.node_id.clone()),
                )?,
                None => HashMap::new(),
            };
            (checkpoints, prompts)
        } else {
            if !agent.workspace_rewind_enabled() {
                self.set_status(
                    "workspace rewind is off; set behavior.workspace_rewind = true and restart Rho",
                );
                return Ok(());
            }
            let prompts = agent
                .stored_session()
                .map(|storage| storage.tree_turn_items())
                .transpose()?
                .unwrap_or_default()
                .into_iter()
                .filter_map(|item| item.first_user_text.map(|prompt| (item.id, prompt)))
                .collect();
            (agent.workspace_checkpoints()?, prompts)
        };
        if checkpoints.is_empty() {
            self.set_status("this session has no workspace checkpoints");
            return Ok(());
        }
        if !invocation.args.is_empty() {
            let matching = checkpoints
                .iter()
                .filter(|checkpoint| checkpoint.node_id.as_str().starts_with(&invocation.args))
                .collect::<Vec<_>>();
            anyhow::ensure!(
                matching.len() == 1,
                "turn '{}' must match exactly one workspace checkpoint",
                invocation.args
            );
            return self.open_rewind_preview(matching[0].node_id.as_str(), agent);
        }

        let items = checkpoints
            .into_iter()
            .rev()
            .map(|checkpoint| {
                let id = checkpoint.node_id.to_string();
                let short_id = &id[..id.len().min(8)];
                let preview = prompts
                    .get(&checkpoint.node_id)
                    .cloned()
                    .unwrap_or_else(|| format!("turn {short_id}"));
                PickerItem {
                    detail: Some(
                        format!(
                            "checkpoint time: {}\ntracked files: {}\noutcome: {:?}",
                            checkpoint.finalized_at, checkpoint.file_count, checkpoint.outcome
                        )
                        .into(),
                    ),
                    badge: (!checkpoint.limitations.is_empty()).then_some(PickerBadge {
                        text: "limited".into(),
                        tone: PickerBadgeTone::Warning,
                    }),
                    ..PickerItem::new(preview, id)
                }
            })
            .collect();
        let picker = UiPicker::rewind_checkpoint("Workspace rewind", items)
            .with_layout(PickerLayout::Overlay)
            .with_overlay_chrome(OverlayChrome {
                nav_label: " REWIND".into(),
                detail_label: Some("CHECKPOINT".into()),
                nav_keys_hint: "↑↓ turns".into(),
            })
            .with_confirm_verb("preview");
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("select workspace checkpoint");
        Ok(())
    }

    pub(super) fn submit_rewind_preview(
        &mut self,
        value: &str,
        agent: &InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.open_rewind_preview(value, agent)
    }

    fn open_rewind_preview(
        &mut self,
        value: &str,
        agent: &InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let node_id = NodeId::from_string(value)?;
        let (checkpoint, plan, workspace) = if self.info.session.daemon.is_some() {
            // Planned here against the files as they are; the daemon plans
            // again when it restores.
            let store = self.daemon_checkpoint_store()?.ok_or_else(|| {
                anyhow::anyhow!("workspace rewind is unavailable for this session")
            })?;
            let root = self.info.runtime.cwd.clone();
            let workspace = WorkspaceOptions { root: root.clone() }
                .build_workspace(&self.info.runtime.added_dirs)?;
            let planned = PlannedRewind::new(&store, &workspace, &node_id)?;
            (planned.checkpoint, planned.plan, root)
        } else {
            let (checkpoint, plan) = agent.preview_workspace_rewind(&node_id)?;
            (checkpoint, plan, agent.workspace_path().to_path_buf())
        };
        let detail = rewind_preview_text(&checkpoint, &plan, &workspace);
        let blocked = plan.entries.iter().any(|entry| {
            matches!(
                entry.classification,
                RestoreClassification::Conflict | RestoreClassification::Unsupported
            )
        });
        let item = PickerItem {
            detail: Some(detail.into()),
            badge: blocked.then_some(PickerBadge {
                text: "partial".into(),
                tone: PickerBadgeTone::Warning,
            }),
            ..PickerItem::new(
                if blocked {
                    "Confirm safe paths; conflicts stay unchanged".into()
                } else {
                    "Confirm workspace rewind".into()
                },
                value.to_string(),
            )
        };
        let picker = UiPicker::confirm_rewind("Confirm workspace rewind", vec![item])
            .with_layout(PickerLayout::Overlay)
            .with_overlay_chrome(OverlayChrome {
                nav_label: " CONFIRM".into(),
                detail_label: Some("RESTORE PREVIEW".into()),
                nav_keys_hint: "Esc cancel".into(),
            })
            .with_confirm_verb("rewind");
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("confirm workspace rewind");
        Ok(())
    }

    pub(super) async fn submit_rewind_confirmation(
        &mut self,
        value: &str,
        _terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let node_id = NodeId::from_string(value)?;
        if self.info.session.daemon.is_some() {
            // The daemon restores the files, says what became of each, and
            // has every client replay the conversation it returned to.
            self.input_ui.set_composer(ComposerMode::Input);
            return self.configure_daemon(SessionChange::RewindWorkspace {
                checkpoint: node_id.to_string(),
            });
        }
        let result = agent.restore_workspace_rewind(&node_id).await?;
        let conversation_selected = result.display.is_some();
        if let Some((before, display)) = result.display {
            let entries = self.transcript_entries(&display);
            self.present_tree_selection(entries, &before, agent);
        } else {
            self.input_ui.set_composer(ComposerMode::Input);
        }
        self.insert_entry(&Entry::Notice(rewind_audit_text(
            &result.audit,
            agent.workspace_path(),
            conversation_selected,
        )));
        if let Some(error) = result.selection_error {
            return Err(error);
        }
        Ok(())
    }
}

fn rewind_preview_text(
    checkpoint: &WorkspaceCheckpoint,
    plan: &RestorePlan,
    workspace: &std::path::Path,
) -> String {
    let short_id = &checkpoint.node_id.as_str()[..checkpoint.node_id.as_str().len().min(8)];
    let mut lines = vec![
        format!("target turn: {short_id}"),
        format!("timestamp: {}", checkpoint.finalized_at),
        "conversation: return to before this turn and preserve the existing branch".into(),
    ];
    for entry in &plan.entries {
        lines.push(format!(
            "{}  {}",
            classification_label(entry.classification),
            rho_tools::compact_display_path(workspace, &entry.path.to_string_lossy())
        ));
    }
    for file in &checkpoint.files {
        if let Some(CheckpointFileBudgetExceeded { asked, limit }) = file.capture_budget_exceeded()
        {
            lines.push(format!(
                "warning  per-file capture budget: limit {limit} bytes, asked {asked} bytes ({})",
                rho_tools::compact_display_path(workspace, &file.path.to_string_lossy())
            ));
        }
    }
    if plan.entries.is_empty() {
        lines.push("skip  no native file changes were captured".into());
    }
    for limitation in &plan.limitations {
        lines.push(format!(
            "warning  {} effects from '{}' cannot be restored",
            limitation_label(&limitation.kind),
            limitation.source
        ));
    }
    lines.join("\n")
}
