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

    /// Opens a confirmation over the active picker, which returns as the
    /// choice's `parent_picker` when it resolves.
    pub(in crate::tui) fn open_choice_over_picker(
        &mut self,
        choice: crate::tui::InlineChoice,
        pending: crate::tui::InlineChoicePending,
        status: &'static str,
    ) -> anyhow::Result<()> {
        let previous = self.input_ui.take_composer();
        let ComposerMode::Picker(parent) = previous else {
            self.input_ui.set_composer(previous);
            anyhow::bail!("confirmation requires an active picker");
        };
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(crate::tui::InlineChoiceModal {
                choice,
                pending,
                parent_picker: Some(Box::new(parent)),
            }));
        self.set_status(status);
        Ok(())
    }

    /// Reopens a confirmation's parent picker with its restore status.
    /// `false` when there was no parent; the caller picks the fallback.
    pub(in crate::tui) fn restore_choice_parent(&mut self, parent: Option<Box<UiPicker>>) -> bool {
        let Some(parent) = parent else {
            return false;
        };
        let status = parent.restore_status();
        self.input_ui.set_composer(ComposerMode::Picker(*parent));
        self.set_status(status);
        true
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
            .map(|item| (picker.action.clone(), item.value.clone()))
    }
}
