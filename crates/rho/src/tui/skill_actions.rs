use super::*;

pub(super) enum SkillCommandAction {
    NotSkill,
    Prompt(TurnPrompt),
    Rejected,
}

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

    pub(super) fn skill_command_action(
        &mut self,
        name: &str,
        model_prompt: String,
        display: String,
        skill_tool_available: bool,
    ) -> anyhow::Result<SkillCommandAction> {
        let Some(name) = name.strip_prefix("skill:") else {
            return Ok(SkillCommandAction::NotSkill);
        };
        if !skill_tool_available {
            self.insert_entry(&Entry::Error(
                "skill commands are unavailable because the active agent has no skill tool".into(),
            ));
            self.status = "skill unavailable".into();
            return Ok(SkillCommandAction::Rejected);
        }
        let Some(skill) = crate::skills::discover(&self.info.runtime.cwd)
            .into_iter()
            .find(|skill| skill.name == name)
        else {
            return Ok(SkillCommandAction::NotSkill);
        };
        Ok(SkillCommandAction::Prompt(
            TurnPrompt::command(model_prompt, display).with_initial_tool_call(
                rho_sdk::model::ToolCall {
                    id: rho_sdk::ToolCallId::new().into_string(),
                    name: "skill".into(),
                    arguments: serde_json::json!({"name": skill.name}),
                },
            ),
        ))
    }
}

#[cfg(test)]
#[path = "skill_actions_tests.rs"]
mod tests;
