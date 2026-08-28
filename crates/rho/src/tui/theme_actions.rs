use super::{theme::Theme, theme_picker, App, ComposerMode, Entry, UiPicker};

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
    /// Uses schemes retained in the picker catalog when the picker was opened.
    pub(super) fn preview_selected_theme_if_active(&mut self) {
        let ComposerMode::Picker(picker) = self.input_ui.composer() else {
            return;
        };
        if !picker.is_theme() {
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
    pub(super) fn cancel_theme_preview(&self) {
        Theme::cancel_preview();
        Theme::clear_picker_catalog();
    }

    pub(super) fn submit_theme_selection(&mut self, value: &str) -> anyhow::Result<()> {
        let save = self.info.services.config_repository.update(|config| {
            config.theme = value.to_string();
            config.theme.clone()
        });
        match save {
            Ok(theme) => {
                Theme::apply_committed(&theme);
                Theme::clear_picker_catalog();
                let label = theme_picker::label_for_theme_id(&theme);
                self.set_status(format!("theme: {label}"));
            }
            Err(error) => {
                Theme::cancel_preview();
                Theme::clear_picker_catalog();
                self.insert_entry(&Entry::Error(format!("could not save theme: {error}")));
                self.set_status("config save failed");
            }
        }
        Ok(())
    }
}
