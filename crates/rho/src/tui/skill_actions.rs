use super::*;

impl App {
    pub(super) fn execute_skills_command(&mut self) -> anyhow::Result<()> {
        let picker = if let Some(options) = &self.daemon_options {
            UiPicker::insert_skill(
                "Loaded skills",
                options
                    .commands
                    .iter()
                    .filter(|command| command.kind == rho_protocol::CommandKind::Skill)
                    .map(|command| PickerItem {
                        detail: Some(command.description.clone().into()),
                        ..PickerItem::new(command.name.clone(), command.name.clone())
                    })
                    .collect(),
            )
        } else {
            skill_picker::skill_picker(crate::skills::discover(
                &self.info.runtime.cwd,
                crate::workspace::ProjectTrusts::of_this_process(),
            ))
        };
        if picker.items.is_empty() {
            self.set_status("no skills loaded");
            return Ok(());
        }

        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("select skill");
        Ok(())
    }
}
