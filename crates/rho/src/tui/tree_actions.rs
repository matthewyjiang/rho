use ratatui::DefaultTerminal;

use crate::session::tree::{NodeId, SessionTreeItemKind};

use super::{
    is_tool_entry, recovered_history_tail, transcript_entries_from_messages, App, ComposerMode,
    Entry, InteractiveRuntime, PickerAction, PickerBadge, PickerBadgeTone, PickerItem, UiPicker,
    ViewModelEvent, RECOVERED_HISTORY_LINE_LIMIT,
};

impl App {
    pub(super) fn execute_tree_command(
        &mut self,
        agent: &InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(storage) = agent.stored_session() else {
            self.insert_entry(&Entry::Notice(
                "no active session tree; send a message first".into(),
            ));
            self.status = "no session tree".into();
            return Ok(());
        };
        let items = storage.tree_items()?;
        if items.is_empty() {
            self.insert_entry(&Entry::Notice(
                "this session tree has no completed turns".into(),
            ));
            self.status = "empty session tree".into();
            return Ok(());
        }
        let selected = items.iter().position(|item| item.active).unwrap_or(0);
        let picker_items = items
            .into_iter()
            .map(|item| {
                let mut connector = item
                    .ancestor_has_next_sibling
                    .iter()
                    .map(|has_next| if *has_next { "│  " } else { "   " })
                    .collect::<String>();
                if item.depth > 0 {
                    connector.push_str(if item.is_last_sibling {
                        "└─ "
                    } else {
                        "├─ "
                    });
                }
                let path = if item.on_active_path { "◆ " } else { "◇ " };
                let kind = if item.kind == SessionTreeItemKind::Compaction {
                    "Compaction"
                } else {
                    "Turn"
                };
                let preview = item
                    .first_user_text
                    .as_deref()
                    .unwrap_or("Compacted context");
                let detail = item.compaction_facts.as_ref().map(|facts| {
                    format!(
                        "{} messages → {} retained",
                        facts.previous_messages, facts.current_messages
                    )
                });
                PickerItem {
                    label: format!("{}{}{}", connector, path, preview),
                    detail: detail.or_else(|| Some(kind.into())),
                    preview: None,
                    badge: item.active.then_some(PickerBadge {
                        text: "active".into(),
                        tone: PickerBadgeTone::Selected,
                    }),
                    value: item.id.to_string(),
                }
            })
            .collect();
        let mut picker = UiPicker::new(
            "Conversation tree",
            "↑/↓ select · type to filter · enter restore · esc cancel",
            picker_items,
            PickerAction::SelectTreeNode,
        )
        .with_confirm_verb("restore");
        picker.selected = selected;
        self.composer = ComposerMode::Picker(picker);
        self.status = "select conversation state".into();
        Ok(())
    }

    pub(super) async fn submit_tree_selection(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let target_id = NodeId::from_string(value)?;
        let storage = agent
            .stored_session()
            .ok_or_else(|| anyhow::anyhow!("active session storage is unavailable"))?;
        let histories = storage.histories_for_node(&target_id)?;
        let entries = transcript_entries_from_messages(&histories.display, &self.info.runtime.cwd);
        let width = terminal.size()?.width as usize;
        let (_, visible_entries) = recovered_history_tail(
            &entries,
            width,
            RECOVERED_HISTORY_LINE_LIMIT,
            self.info.runtime.max_tool_output_lines,
        );
        agent.select_tree_node(storage, &target_id).await?;

        self.info.session.recovered_messages = histories.display.clone();
        self.composer = ComposerMode::Input;
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.command_palette_dismissed = false;
        self.reset_streams();
        self.goal = None;
        self.reset_usage();
        self.current_context = None;
        self.transcript = visible_entries;
        self.markdown_images.clear();
        self.mark_markdown_images_dirty_from(0);
        self.history_lines.invalidate_from(0);
        self.last_inserted_was_tool = self.transcript.last().is_some_and(is_tool_entry);
        self.scroll_history_to_bottom();
        if let Some(context) = agent.take_context_usage() {
            self.record_agent_event(ViewModelEvent::ContextUsage(context));
        }
        self.insert_runtime_notices(agent);
        self.insert_entry(&Entry::Notice(format!(
            "restored conversation state {}",
            &target_id.as_str()[..target_id.as_str().len().min(8)]
        )));
        self.status = "conversation state restored".into();
        Ok(())
    }
}
