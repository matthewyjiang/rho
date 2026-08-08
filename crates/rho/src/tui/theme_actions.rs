use super::{theme::Theme, theme_picker, App, ComposerMode, Entry, PickerAction, UiPicker};

impl App {
    pub(super) fn open_theme_picker(&mut self) -> anyhow::Result<()> {
        self.open_theme_picker_with(|app, picker| {
            app.input_ui.set_composer(ComposerMode::Picker(picker));
        })
    }

    pub(super) fn open_theme_picker_from_config(&mut self) -> anyhow::Result<()> {
        self.open_theme_picker_with(|app, picker| {
            app.open_child_picker(picker);
        })
    }

    fn open_theme_picker_with(
        &mut self,
        attach: impl FnOnce(&mut Self, UiPicker),
    ) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        let picker = theme_picker::theme_picker(&config.theme);
        if let Some(item) = picker.selected_item() {
            Theme::preview(&item.value);
        }
        attach(self, picker);
        self.set_status("preview themes · enter applies");
        Ok(())
    }

    /// Preview the highlighted theme row after selection changes (not during draw).
    pub(super) fn preview_selected_theme_if_active(&mut self) {
        let ComposerMode::Picker(picker) = self.input_ui.composer() else {
            return;
        };
        if picker.action != PickerAction::SelectTheme {
            return;
        }
        let Some(item) = picker.selected_item() else {
            return;
        };
        if Theme::active_id() != item.value {
            Theme::preview(&item.value);
        }
    }

    /// Leave the theme picker without applying: restore committed colors.
    pub(super) fn cancel_theme_preview_if_leaving(&self, action: PickerAction) {
        if action == PickerAction::SelectTheme {
            Theme::cancel_preview();
        }
    }

    pub(super) fn submit_theme_selection(&mut self, value: &str) -> anyhow::Result<()> {
        let save = self.info.services.config_repository.update(|config| {
            config.theme = value.to_string();
            config.theme.clone()
        });
        match save {
            Ok(theme) => {
                Theme::apply_committed(&theme);
                let label = theme_picker::label_for_theme_id(&theme);
                self.set_status(format!("theme: {label}"));
            }
            Err(error) => {
                Theme::cancel_preview();
                self.insert_entry(&Entry::Error(format!("could not save theme: {error}")));
                self.set_status("config save failed");
            }
        }
        Ok(())
    }
}
