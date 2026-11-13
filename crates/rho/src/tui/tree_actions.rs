use rho_protocol::SessionChange;

use crate::tui::DefaultTerminal;

use crate::session::tree::{NodeId, SessionTreeItem, SessionTreeItemKind};

use super::{
    picker::OverlayChrome, App, ComposerMode, PickerBadge, PickerBadgeTone, PickerItem,
    PickerLayout, UiPicker,
};

pub(super) fn tree_picker(items: Vec<SessionTreeItem>) -> UiPicker {
    let selected = items.iter().position(|item| item.active).unwrap_or(0);
    let picker_items = items.into_iter().map(tree_item).collect();
    let mut picker = UiPicker::tree("Conversation tree", picker_items)
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
        badge: item.active.then_some(PickerBadge {
            text: "active".into(),
            tone: PickerBadgeTone::Selected,
        }),
        ..PickerItem::new(tree_label(&item, &preview), item.id.to_string())
    }
}

fn tree_preview(item: &SessionTreeItem) -> String {
    match item.kind {
        SessionTreeItemKind::SessionStart => "Session start".into(),
        SessionTreeItemKind::Turn => item
            .first_user_text
            .clone()
            .unwrap_or_else(|| "turn".into()),
        SessionTreeItemKind::Compaction => item.compaction_facts.as_ref().map_or_else(
            || "Compacted context".into(),
            |facts| {
                format!(
                    "Compacted context ({} → {} messages)",
                    facts.previous_messages, facts.current_messages
                )
            },
        ),
    }
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
    pub(super) fn execute_tree_command(&mut self) -> anyhow::Result<()> {
        if self.info.session.no_save {
            self.set_status("conversation tree unavailable with --no-save");
            return Ok(());
        }
        // The daemon holds the session open; its tree is read from the file
        // both share.
        let Some(id) = self.current_session_id() else {
            self.set_status("no active session tree; send a message first");
            return Ok(());
        };
        let items = crate::session::Session::tree_turn_items_by_id(&self.info.runtime.cwd, &id)?;
        if items.is_empty() {
            self.set_status("this session tree has no turns");
            return Ok(());
        }
        self.input_ui
            .set_composer(ComposerMode::Picker(tree_picker(items)));
        self.set_status("select conversation state");
        Ok(())
    }

    /// The daemon moves the session to the selected node and has every
    /// client replay the conversation as it then stands.
    pub(super) async fn submit_tree_selection(
        &mut self,
        value: &str,
        _terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let target_id = NodeId::from_string(value)?;
        self.input_ui.set_composer(ComposerMode::Input);
        self.configure_daemon(SessionChange::SelectNode {
            node: target_id.to_string(),
        })
    }
}
