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

    pub(super) fn execute_skill_command(
        &mut self,
        name: &str,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(name) = name.strip_prefix("skill:") else {
            return Ok(false);
        };
        let Some(skill) = crate::skills::discover(&self.info.runtime.cwd)
            .into_iter()
            .find(|skill| skill.name == name)
        else {
            return Ok(false);
        };

        self.ensure_session(agent)?;
        agent.load_skill(
            &skill,
            self.info
                .services
                .config_repository
                .load()?
                .max_output_bytes,
        )?;
        self.insert_entry(&Entry::Notice(format!(
            "loaded skill {} from {}",
            skill.name, skill.source
        )));
        self.status = format!("loaded skill {}", skill.name);
        Ok(true)
    }
}
