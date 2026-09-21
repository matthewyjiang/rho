//! Read-only output selection from the displayed conversation, including unsaved text.

use super::{picker::OverlayChrome, App, ComposerMode, Entry, PickerItem, PickerLayout, UiPicker};

impl App {
    pub(super) fn execute_copy_command(&mut self) -> anyhow::Result<()> {
        let mut items: Vec<PickerItem> = Vec::new();
        let mut prompt = None;
        let mut turn = 0;
        let mut previous_output: Option<usize> = None;
        for entry in self.history.entries() {
            match entry {
                Entry::User(text) => {
                    turn += 1;
                    prompt = Some(format!(
                        "{turn}. {}",
                        text.split_whitespace().collect::<Vec<_>>().join(" ")
                    ));
                    previous_output = None;
                }
                Entry::Assistant(assistant) if !assistant.text.trim().is_empty() => {
                    if let Some(index) = previous_output {
                        items[index].label = items[index].label.replacen("└─", "├─", 1);
                    }
                    let preview = assistant.text.trim().lines().next().unwrap_or_default();
                    previous_output = Some(items.len());
                    items.push(PickerItem {
                        section: prompt.clone(),
                        label: format!("└─ {preview}"),
                        detail: Some(assistant.text.clone()),
                        preview: None,
                        badge: None,
                        // Snapshot the payload so incoming stream updates cannot change
                        // which text Enter copies while the picker is open.
                        value: assistant.text.clone(),
                        selection_verb: None,
                        allow_filter_completion: false,
                    });
                }
                Entry::Assistant(_)
                | Entry::Reasoning(_)
                | Entry::Tool(_)
                | Entry::Notice(_)
                | Entry::RuntimeInfo(_)
                | Entry::Changelog(_)
                | Entry::Error(_) => {}
            }
        }
        if items.is_empty() {
            self.set_status("no assistant message to copy");
            return Ok(());
        }
        let selected = items.len() - 1;
        let mut picker = UiPicker::copy_output("Copy output", items)
            .with_layout(PickerLayout::Overlay)
            .with_overlay_chrome(OverlayChrome {
                nav_label: " TREE".into(),
                detail_label: Some(" OUTPUT".into()),
                nav_keys_hint: "↑↓ outputs".into(),
            });
        picker.selected = selected;
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("select output to copy");
        Ok(())
    }
}
