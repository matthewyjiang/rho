use ratatui::DefaultTerminal;

use crate::session::tree::{NodeId, SessionTreeItem};

use super::{
    message_history::transcript_entries_from_messages, picker_overlay::OverlayChrome, App,
    ComposerMode, InteractiveRuntime, PickerAction, PickerBadge, PickerBadgeTone, PickerItem,
    PickerLayout, UiPicker, ViewModelEvent,
};

pub(super) fn tree_picker(items: Vec<SessionTreeItem>) -> UiPicker {
    let selected = items.iter().position(|item| item.active).unwrap_or(0);
    let picker_items = items.into_iter().map(tree_item).collect();
    let mut picker = UiPicker::new(
        "Conversation tree",
        picker_items,
        PickerAction::SelectTreeNode,
    )
    .with_layout(PickerLayout::Overlay)
    .with_overlay_chrome(OverlayChrome {
        nav_label: " TREE".into(),
        detail_label: None,
        nav_keys_hint: "↑↓ turns".into(),
    })
    .with_confirm_verb("restore");
    picker.selected = selected;
    picker
}

fn tree_item(item: SessionTreeItem) -> PickerItem {
    let preview = tree_preview(&item);
    PickerItem {
        section: None,
        label: tree_label(&item, &preview),
        detail: None,
        preview: None,
        badge: item.active.then_some(PickerBadge {
            text: "active".into(),
            tone: PickerBadgeTone::Selected,
        }),
        value: item.id.to_string(),
        selection_verb: None,
    }
}

fn tree_preview(item: &SessionTreeItem) -> String {
    if let Some(text) = item.first_user_text.as_deref() {
        return text.to_string();
    }
    if let Some(facts) = item.compaction_facts.as_ref() {
        return format!(
            "Compacted context ({} → {} messages)",
            facts.previous_messages, facts.current_messages
        );
    }
    "Compacted context".into()
}

fn tree_label(item: &SessionTreeItem, preview: &str) -> String {
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
    format!("{connector}{path}{preview}")
}

impl App {
    pub(super) fn execute_tree_command(
        &mut self,
        agent: &InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(storage) = agent.stored_session() else {
            self.set_status("no active session tree; send a message first");
            return Ok(());
        };
        let items = storage.tree_items()?;
        if items.is_empty() {
            self.set_status("this session tree has no completed turns");
            return Ok(());
        }
        self.input_ui
            .set_composer(ComposerMode::Picker(tree_picker(items)));
        self.set_status("select conversation state");
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
        let size = terminal.size()?;
        self.note_terminal_geometry(size.width as usize, size.height as usize);
        agent.select_tree_node(storage, &target_id).await?;

        self.input_ui.set_composer(ComposerMode::Input);
        self.input_ui.clear_text();
        self.input_ui.clear_paste_segments();
        self.input_ui.set_shell_mode(None);
        self.input_ui.set_cursor(0);
        self.input_ui.set_command_palette_dismissed(false);
        self.reset_streams();
        self.goal = None;
        self.reset_usage();
        self.usage.current_context = None;
        self.history.set_entries(entries);
        self.history.images_mut().clear();
        self.scroll_history_to_bottom();
        if let Some(context) = agent.take_context_usage() {
            self.record_agent_event(ViewModelEvent::ContextUsage(context));
        }
        self.insert_runtime_notices(agent);
        self.set_status(format!(
            "restored conversation state {}",
            &target_id.as_str()[..target_id.as_str().len().min(8)]
        ));
        Ok(())
    }
}
