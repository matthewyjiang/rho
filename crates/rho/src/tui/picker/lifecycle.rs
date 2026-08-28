//! Open, close, and cursor restore for an active picker.

use ratatui::DefaultTerminal;

use super::{overlay_layout::picker_overlay_layout, PickerAction, UiPicker};
use crate::tui::{App, ComposerMode};

impl App {
    pub(in crate::tui) fn clamp_overlay_detail_scroll(&mut self, terminal: &DefaultTerminal) {
        let Ok(size) = terminal.size() else {
            return;
        };
        let ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
            return;
        };
        if !picker.has_scrollable_detail() {
            return;
        }
        let layout = picker_overlay_layout(
            ratatui::layout::Rect::new(0, 0, size.width, size.height),
            picker.overlay_sizing(),
        );
        if let Some(viewport) = layout.detail_viewport() {
            picker.clamp_detail_scroll(viewport);
        }
    }

    pub(in crate::tui) fn open_child_picker(&mut self, child: UiPicker) {
        let previous = self.input_ui.take_composer();
        let ComposerMode::Picker(parent) = previous else {
            unreachable!("child picker requires an active parent picker")
        };
        self.set_status_quiet(child.title.clone());
        self.input_ui
            .set_composer(ComposerMode::Picker(child.with_parent(parent)));
    }

    pub(in crate::tui) fn pop_picker_level(&mut self) -> bool {
        let parent = match self.input_ui.composer_mut() {
            ComposerMode::Picker(picker) => picker.take_parent(),
            _ => None,
        };
        let Some(parent) = parent else {
            return false;
        };
        self.set_status_quiet(parent.title.clone());
        self.input_ui.set_composer(ComposerMode::Picker(parent));
        true
    }

    pub(in crate::tui) fn picker_space_confirms_selection(&self) -> bool {
        matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.space_confirms_selection()
        )
    }

    pub(in crate::tui) fn restore_picker_position(
        picker: &mut UiPicker,
        selected_value: &str,
        filter: String,
    ) {
        picker.filter = filter;
        if let Some(index) = picker
            .items
            .iter()
            .position(|item| item.value == selected_value)
        {
            picker.selected = index;
            if picker.selected_item().is_some() {
                return;
            }
        }
        picker.filter.clear();
        if let Some(index) = picker
            .items
            .iter()
            .position(|item| item.value == selected_value)
        {
            picker.selected = index;
        } else {
            picker.select_first_match();
        }
    }

    #[cfg(test)]
    pub(in crate::tui) fn active_picker_value(&self) -> Option<String> {
        self.active_picker_selection().map(|(_, value)| value)
    }

    pub(in crate::tui) fn active_picker_selection(&self) -> Option<(PickerAction, String)> {
        let ComposerMode::Picker(picker) = self.input_ui.composer() else {
            return None;
        };
        picker
            .selected_item()
            .map(|item| (picker.action, item.value.clone()))
    }
}
