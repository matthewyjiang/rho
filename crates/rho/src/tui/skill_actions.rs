use super::*;

pub(super) enum SkillCommandAction {
    NotSkill,
    Prompt(Box<TurnPrompt>),
    Rejected,
}

impl App {
    pub(super) fn execute_skills_command(&mut self) -> anyhow::Result<()> {
        let picker = skill_picker::skill_picker(crate::skills::discover(&self.info.runtime.cwd));
        if picker.items.is_empty() {
            self.set_status("no skills loaded");
            return Ok(());
        }

        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("select skill");
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
            self.set_status("skill unavailable");
            return Ok(SkillCommandAction::Rejected);
        }
        let Some(skill) = crate::skills::discover(&self.info.runtime.cwd)
            .into_iter()
            .find(|skill| skill.name == name)
        else {
            return Ok(SkillCommandAction::NotSkill);
        };
        Ok(SkillCommandAction::Prompt(Box::new(
            TurnPrompt::command(model_prompt, display).with_skill(skill.name),
        )))
    }

    /// Also loads the skills a typed turn mentions as `$<name>`, when the
    /// active agent can load skills. Generated turns, such as goal
    /// continuations and `/init`, skip this so their text stays prose.
    pub(super) fn load_mentioned_skills(&self, turn: &mut TurnPrompt, agent: &InteractiveRuntime) {
        if agent.has_tool(crate::skills::mention::SKILL_TOOL) {
            turn.load_mentioned_skills(|| crate::skills::discover(&self.info.runtime.cwd));
        }
    }

    /// A queued follow-up as a turn that loads the skills it mentions.
    pub(super) fn queued_turn_submission(
        &self,
        prompt: QueuedPrompt,
        agent: &InteractiveRuntime,
    ) -> super::send_confirm::SendSubmission {
        let mut turn = TurnPrompt::standard(prompt.prompt, prompt.display_prompt);
        self.load_mentioned_skills(&mut turn, agent);
        super::send_confirm::SendSubmission::turn(turn, prompt.media, prompt.paste_segments)
    }
}

#[cfg(test)]
#[path = "skill_actions_tests.rs"]
mod tests;
