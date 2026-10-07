//! Searchable prompt history, opened with `search_prompt_history` (Ctrl+R by
//! default). Choosing a prompt recalls it into the composer exactly as Up/Down
//! would, so Down still steps forward and back to the saved draft.

use std::collections::HashSet;

use super::{picker::OverlayChrome, App, ComposerMode, PickerItem, PickerLayout, UiPicker};

/// Picker rows for `history` (oldest first), newest first with repeats
/// collapsed into their latest use. Filtering keeps this order, so the most
/// recent match is highlighted first, like a shell's reverse search. Rows
/// carry no detail, so the overlay is a single full-width list; the filter
/// still matches the full prompt through `value`.
fn prompt_history_items(history: &[String]) -> Vec<PickerItem> {
    let mut seen = HashSet::new();
    history
        .iter()
        .rev()
        .filter(|prompt| seen.insert(prompt.as_str()))
        .map(|prompt| PickerItem {
            allow_filter_completion: false,
            ..PickerItem::new(
                prompt.split_whitespace().collect::<Vec<_>>().join(" "),
                prompt.clone(),
            )
        })
        .collect()
}

impl App {
    pub(super) fn open_prompt_history_search(&mut self) {
        let items = prompt_history_items(self.input_ui.history());
        if items.is_empty() {
            self.notify_status("no prompt history yet");
            return;
        }
        let picker = UiPicker::prompt_history("Prompt history", items)
            .with_layout(PickerLayout::Overlay)
            .with_overlay_chrome(OverlayChrome {
                nav_label: " PROMPTS".into(),
                detail_label: None,
                nav_keys_hint: "↑↓ prompts".into(),
            })
            .with_confirm_verb("recall");
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("search prompt history");
    }

    /// Recalls the chosen prompt at its latest history position.
    pub(super) fn recall_searched_prompt(&mut self, prompt: &str) {
        if let Some(index) = self
            .input_ui
            .history()
            .iter()
            .rposition(|entry| entry == prompt)
        {
            self.recall_history_entry(index);
        }
        self.set_status("prompt recalled");
    }
}

#[cfg(test)]
#[path = "prompt_history_search_tests.rs"]
mod tests;
