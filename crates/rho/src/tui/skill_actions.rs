use super::*;

impl App {
    pub(super) fn execute_skills_command(&mut self) -> anyhow::Result<()> {
        let picker = skill_picker::skill_picker(crate::skills::discover(&self.info.runtime.cwd));
        if picker.items.is_empty() {
            self.insert_entry(&Entry::Notice("no skills loaded".into()));
            self.status = "skills".into();
            return Ok(());
        }

        self.composer = ComposerMode::Picker(picker);
        self.status = "select skill".into();
        Ok(())
    }

    pub(super) fn expand_skill_command(
        &self,
        name: &str,
        additional_instructions: &str,
    ) -> anyhow::Result<Option<String>> {
        let Some(name) = name.strip_prefix("skill:") else {
            return Ok(None);
        };
        let Some(skill) = crate::skills::discover(&self.info.runtime.cwd)
            .into_iter()
            .find(|skill| skill.name == name)
        else {
            return Ok(None);
        };
        let max_output_bytes = self
            .info
            .services
            .config_repository
            .load()?
            .max_output_bytes;

        Ok(Some(crate::skills::format_invocation(
            &skill,
            additional_instructions,
            max_output_bytes,
        )))
    }
}
