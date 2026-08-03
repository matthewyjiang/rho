use std::collections::HashMap;

use ratatui::DefaultTerminal;

use crate::{
    commands::CommandInvocation,
    session::{
        tree::NodeId,
        workspace_checkpoint::{
            RestoreAudit, RestoreClassification, RestorePlan, WorkspaceCheckpoint,
        },
    },
};

use super::{
    picker_overlay::OverlayChrome, App, ComposerMode, Entry, InteractiveRuntime, PickerAction,
    PickerBadge, PickerBadgeTone, PickerItem, PickerLayout, UiPicker,
};

impl App {
    pub(super) fn execute_rewind_command(
        &mut self,
        invocation: CommandInvocation,
        agent: &InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if !agent.workspace_rewind_enabled() {
            self.set_status(
                "workspace rewind is experimental; set behavior.experimental_workspace_rewind = true and restart Rho",
            );
            return Ok(());
        }
        let checkpoints = agent.workspace_checkpoints()?;
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

        let previews = agent
            .stored_session()
            .map(|storage| storage.tree_items())
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| {
                item.first_user_text
                    .map(|preview| (item.id.to_string(), preview))
            })
            .collect::<HashMap<_, _>>();
        let items = checkpoints
            .into_iter()
            .rev()
            .map(|checkpoint| {
                let id = checkpoint.node_id.to_string();
                let short_id = &id[..id.len().min(8)];
                let preview = previews
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("turn {short_id}"));
                PickerItem {
                    section: None,
                    label: preview,
                    detail: Some(format!(
                        "checkpoint time: {}\ntracked files: {}\noutcome: {:?}",
                        checkpoint.finalized_at,
                        checkpoint.files.len(),
                        checkpoint.outcome
                    )),
                    preview: None,
                    badge: (!checkpoint.limitations.is_empty()).then_some(PickerBadge {
                        text: "limited".into(),
                        tone: PickerBadgeTone::Warning,
                    }),
                    value: id,
                    selection_verb: None,
                }
            })
            .collect();
        let picker = UiPicker::new(
            "Workspace rewind",
            items,
            PickerAction::SelectRewindCheckpoint,
        )
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
        let (checkpoint, plan) = agent.preview_workspace_rewind(&node_id)?;
        let detail = rewind_preview_text(&checkpoint, &plan, agent.workspace_path());
        let blocked = plan.entries.iter().any(|entry| {
            matches!(
                entry.classification,
                RestoreClassification::Conflict | RestoreClassification::Unsupported
            )
        });
        let item = PickerItem {
            section: None,
            label: if blocked {
                "Confirm safe paths; conflicts stay unchanged".into()
            } else {
                "Confirm workspace rewind".into()
            },
            detail: Some(detail),
            preview: None,
            badge: blocked.then_some(PickerBadge {
                text: "partial".into(),
                tone: PickerBadgeTone::Warning,
            }),
            value: value.to_string(),
            selection_verb: None,
        };
        let picker = UiPicker::new(
            "Confirm workspace rewind",
            vec![item],
            PickerAction::ConfirmRewindCheckpoint,
        )
        .with_layout(PickerLayout::Overlay)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " CONFIRM".into(),
            detail_label: Some("RESTORE PREVIEW".into()),
            nav_keys_hint: "esc cancels".into(),
        })
        .with_confirm_verb("rewind");
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("confirm workspace rewind");
        Ok(())
    }

    pub(super) async fn submit_rewind_confirmation(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let node_id = NodeId::from_string(value)?;
        let audit = agent.restore_workspace_rewind(&node_id).await?;
        let incomplete = audit.entries.iter().any(|entry| {
            entry.error.is_some()
                || matches!(
                    entry.classification,
                    RestoreClassification::Conflict | RestoreClassification::Unsupported
                )
        });
        let selection_error = if incomplete {
            self.input_ui.set_composer(ComposerMode::Input);
            None
        } else {
            self.submit_tree_selection(value, terminal, agent)
                .await
                .err()
        };
        let conversation_selected = !incomplete && selection_error.is_none();
        self.insert_entry(&Entry::Notice(rewind_audit_text(
            &audit,
            agent.workspace_path(),
            conversation_selected,
        )));
        if let Some(error) = selection_error {
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
        "conversation: select this state and preserve the existing branch".into(),
    ];
    for entry in &plan.entries {
        lines.push(format!(
            "{}  {}",
            classification_label(entry.classification),
            rho_tools::compact_display_path(workspace, &entry.path.to_string_lossy())
        ));
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

fn rewind_audit_text(
    audit: &RestoreAudit,
    workspace: &std::path::Path,
    conversation_selected: bool,
) -> String {
    let mut lines = vec![if conversation_selected {
        "workspace rewind audit; conversation state selected".into()
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

fn classification_label(classification: RestoreClassification) -> &'static str {
    match classification {
        RestoreClassification::Create => "create",
        RestoreClassification::Modify => "modify",
        RestoreClassification::Delete => "delete",
        RestoreClassification::Conflict => "conflict",
        RestoreClassification::Unsupported => "unsupported",
        RestoreClassification::Skipped => "skip",
    }
}

fn limitation_label(
    kind: &crate::session::workspace_checkpoint::UntrackedEffectKind,
) -> &'static str {
    match kind {
        crate::session::workspace_checkpoint::UntrackedEffectKind::ShellCommand => "shell",
        crate::session::workspace_checkpoint::UntrackedEffectKind::UntrackedMutatingTool => {
            "process"
        }
        crate::session::workspace_checkpoint::UntrackedEffectKind::ExternalEffect => "external",
    }
}
